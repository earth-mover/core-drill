//! Schema-driven flatbuffer walker.
//!
//! Every read is bounds-checked against the buffer, so a corrupt or
//! malicious file produces an error rather than undefined behavior, even if
//! it failed verification. Offsets in flatbuffers only point forward, so
//! walking always terminates; the depth limit guards the stack should a future
//! schema add recursive types.

use color_eyre::eyre::{Result, bail, eyre};
use flatbuffers_reflection::reflection::{BaseType, Field, Object, Schema, Type};

use super::annotate::Annotator;
use super::value::RawValue;

/// One step of an `--at` selector such as `arrays/0/refs/10..20`.
#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Field(String),
    Index(usize),
    Range(usize, usize),
}

impl std::fmt::Display for Seg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Seg::Field(name) => f.write_str(name),
            Seg::Index(i) => write!(f, "{i}"),
            Seg::Range(a, b) if *b == usize::MAX => write!(f, "{a}.."),
            Seg::Range(a, b) => write!(f, "{a}..{b}"),
        }
    }
}

pub fn parse_selector(s: &str) -> Result<Vec<Seg>> {
    s.split('/')
        .filter(|p| !p.is_empty())
        .map(|p| {
            if let Some((a, b)) = p.split_once("..") {
                let start = if a.is_empty() { 0 } else { a.parse()? };
                let end = if b.is_empty() { usize::MAX } else { b.parse()? };
                if end < start {
                    bail!("empty range {p}");
                }
                Ok(Seg::Range(start, end))
            } else if let Ok(i) = p.parse() {
                Ok(Seg::Index(i))
            } else {
                Ok(Seg::Field(p.to_string()))
            }
        })
        .collect()
}

/// Strip the `generated.` namespace from schema object names.
pub fn short_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

pub struct Walker<'a> {
    buf: &'a [u8],
    schema: Schema<'a>,
    annotator: &'a Annotator<'a>,
    /// Elements shown per vector; 0 means no limit.
    max_items: usize,
}

impl<'a> Walker<'a> {
    pub fn new(
        buf: &'a [u8],
        schema: Schema<'a>,
        annotator: &'a Annotator<'a>,
        max_items: usize,
    ) -> Self {
        Self {
            buf,
            schema,
            annotator,
            max_items,
        }
    }

    pub fn root_table(&self) -> Result<(Object<'a>, usize)> {
        let root = self
            .schema
            .root_table()
            .ok_or_else(|| eyre!("schema has no root table"))?;
        Ok((root, self.u32_at(0)? as usize))
    }

    pub fn walk_root(&self, sel: &[Seg]) -> Result<RawValue> {
        let (root, loc) = self.root_table()?;
        self.table(root, loc, sel, 0)
    }

    /// Position of a present field's value inside a table, or `None` if the
    /// vtable omits it.
    pub fn field_pos(&self, table_loc: usize, field: &Field) -> Result<Option<usize>> {
        let vtable = table_loc as i64 - self.i32_at(table_loc)? as i64;
        let vtable = usize::try_from(vtable).map_err(|_| eyre!("vtable offset out of range"))?;
        let vtable_len = self.u16_at(vtable)? as usize;
        let slot = field.offset() as usize;
        if slot + 2 > vtable_len {
            return Ok(None);
        }
        match self.u16_at(vtable + slot)? {
            0 => Ok(None),
            off => Ok(Some(table_loc + off as usize)),
        }
    }

    pub fn bytes_field(
        &self,
        obj: Object<'a>,
        table_loc: usize,
        name: &str,
    ) -> Result<Option<&'a [u8]>> {
        let Some(field) = find_field(&obj, name) else {
            return Ok(None);
        };
        let Some(pos) = self.field_pos(table_loc, &field)? else {
            return Ok(None);
        };
        let vec = self.follow(pos)?;
        let len = self.u32_at(vec)? as usize;
        Ok(Some(self.slice(vec + 4, len)?))
    }

    fn table(&self, obj: Object<'a>, loc: usize, sel: &[Seg], depth: usize) -> Result<RawValue> {
        if depth > 64 {
            bail!("nesting deeper than 64 tables");
        }
        let type_name = short_name(obj.name());
        if let Some((first, rest)) = sel.split_first() {
            let Seg::Field(name) = first else {
                bail!("{type_name} is a table; select a field name, not {first:?}");
            };
            let field = find_field(&obj, name).ok_or_else(|| {
                eyre!(
                    "{type_name} has no field '{name}' (fields: {})",
                    field_names(&obj).join(", ")
                )
            })?;
            let pos = self
                .field_pos(loc, &field)?
                .ok_or_else(|| eyre!("field '{name}' is not set in this {type_name}"))?;
            return self.field_value(&obj, loc, &field, pos, rest, depth);
        }

        let fields = sorted_fields(&obj);
        let mut out = Vec::with_capacity(fields.len());
        for field in fields {
            // The union's `_type` discriminant is shown as the union value's type name.
            if field.type_().base_type() == BaseType::UType {
                continue;
            }
            let Some(pos) = self.field_pos(loc, &field)? else {
                continue;
            };
            let value = self.field_value(&obj, loc, &field, pos, &[], depth)?;
            out.push((field.name().to_string(), value));
        }
        Ok(RawValue::Table {
            type_name: type_name.to_string(),
            fields: out,
        })
    }

    fn field_value(
        &self,
        obj: &Object<'a>,
        table_loc: usize,
        field: &Field<'a>,
        pos: usize,
        sel: &[Seg],
        depth: usize,
    ) -> Result<RawValue> {
        let ty = field.type_();
        let owner = short_name(obj.name());
        let fname = field.name();
        let leaf = |v: RawValue| -> Result<RawValue> {
            if let Some(s) = sel.first() {
                bail!("'{fname}' is a scalar; cannot select {s:?} inside it");
            }
            Ok(v)
        };
        match ty.base_type() {
            BaseType::String => {
                let s = self.string_at(self.follow(pos)?)?;
                leaf(self.annotator.string(owner, fname, s))
            }
            BaseType::Obj => {
                let child = self.object(ty.index())?;
                if child.is_struct() {
                    self.structure(child, pos, owner, fname, sel)
                } else {
                    self.table(child, self.follow(pos)?, sel, depth + 1)
                }
            }
            BaseType::Union => {
                let tag_field = find_field(obj, &format!("{fname}_type"))
                    .ok_or_else(|| eyre!("union '{fname}' has no _type field"))?;
                let tag = match self.field_pos(table_loc, &tag_field)? {
                    Some(p) => self.u8_at(p)? as i64,
                    None => 0,
                };
                if tag == 0 {
                    bail!("union '{fname}' has a value but its type is NONE");
                }
                let enum_def = self.schema.enums().get(ty.index() as usize);
                let variant = enum_def
                    .values()
                    .iter()
                    .find(|v| v.value() == tag)
                    .and_then(|v| v.union_type())
                    .ok_or_else(|| eyre!("union '{fname}' has unknown type tag {tag}"))?;
                let child = self.object(variant.index())?;
                self.table(child, self.follow(pos)?, sel, depth + 1)
            }
            BaseType::Vector => self.vector(&ty, self.follow(pos)?, owner, fname, sel, depth),
            _ => leaf(self.scalar_field(&ty, pos, owner, fname)?),
        }
    }

    fn vector(
        &self,
        ty: &Type<'a>,
        loc: usize,
        owner: &str,
        fname: &str,
        sel: &[Seg],
        depth: usize,
    ) -> Result<RawValue> {
        let len = self.u32_at(loc)? as usize;
        let data = loc + 4;
        let elem = ty.element();

        if matches!(elem, BaseType::UByte | BaseType::Byte) {
            if let Some(s) = sel.first() {
                bail!("'{fname}' is a byte vector; cannot select {s:?} inside it");
            }
            return Ok(self.annotator.bytes(owner, fname, self.slice(data, len)?));
        }

        let struct_obj = (elem == BaseType::Obj)
            .then(|| self.object(ty.index()))
            .transpose()?
            .filter(|o| o.is_struct());
        let stride = match &struct_obj {
            Some(o) => o.bytesize() as usize,
            None if matches!(elem, BaseType::Obj | BaseType::String) => 4,
            None => {
                scalar_size(elem).ok_or_else(|| eyre!("unsupported vector element in '{fname}'"))?
            }
        };
        let element = |i: usize, rest: &[Seg]| -> Result<RawValue> {
            let p = data + i * stride;
            match (elem, &struct_obj) {
                (_, Some(o)) => self.structure(*o, p, owner, fname, rest),
                (BaseType::Obj, None) => {
                    self.table(self.object(ty.index())?, self.follow(p)?, rest, depth + 1)
                }
                (BaseType::String, None) => Ok(RawValue::Str(self.string_at(self.follow(p)?)?)),
                _ => self.scalar(elem, p),
            }
        };

        let is_scalar = struct_obj.is_none() && !matches!(elem, BaseType::Obj | BaseType::String);
        let (start, end) = match sel.first() {
            Some(Seg::Index(i)) => {
                if *i >= len {
                    bail!("index {i} out of range for '{fname}' (len {len})");
                }
                return element(*i, &sel[1..]);
            }
            Some(Seg::Range(a, b)) => {
                if sel.len() > 1 {
                    bail!("a range must be the last selector step");
                }
                (*a.min(&len), *b.min(&len))
            }
            Some(Seg::Field(f)) => bail!("'{fname}' is a vector; select an index, not '{f}'"),
            None if is_scalar || self.max_items == 0 => (0, len),
            None => (0, len.min(self.max_items)),
        };
        let items = (start..end)
            .map(|i| element(i, &[]))
            .collect::<Result<Vec<_>>>()?;
        if is_scalar {
            Ok(RawValue::Scalars(items))
        } else {
            Ok(RawValue::Vector { len, start, items })
        }
    }

    fn structure(
        &self,
        obj: Object<'a>,
        loc: usize,
        owner: &str,
        fname: &str,
        sel: &[Seg],
    ) -> Result<RawValue> {
        let type_name = short_name(obj.name());
        if matches!(type_name, "ObjectId12" | "ObjectId8") {
            if let Some(s) = sel.first() {
                bail!("'{fname}' is an ID; cannot select {s:?} inside it");
            }
            let bytes = self.slice(loc, obj.bytesize() as usize)?;
            return Ok(self.annotator.object_id(owner, fname, bytes));
        }
        let fields = sorted_fields(&obj);
        if let Some((Seg::Field(name), rest)) = sel.split_first() {
            let field = fields
                .iter()
                .find(|f| f.name() == name)
                .ok_or_else(|| eyre!("{type_name} has no field '{name}'"))?;
            if !rest.is_empty() {
                bail!("cannot select inside struct field '{name}'");
            }
            return self.struct_field(type_name, field, loc + field.offset() as usize);
        }
        if let Some(s) = sel.first() {
            bail!("{type_name} is a struct; select a field name, not {s:?}");
        }
        let fields = fields
            .iter()
            .map(|f| {
                let v = self.struct_field(type_name, f, loc + f.offset() as usize)?;
                Ok((f.name().to_string(), v))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(RawValue::Table {
            type_name: type_name.to_string(),
            fields,
        })
    }

    fn struct_field(&self, owner: &str, field: &Field<'a>, pos: usize) -> Result<RawValue> {
        let ty = field.type_();
        match ty.base_type() {
            BaseType::Obj => {
                self.structure(self.object(ty.index())?, pos, owner, field.name(), &[])
            }
            BaseType::Array => {
                let elem = ty.element();
                let size = scalar_size(elem).ok_or_else(|| eyre!("unsupported array element"))?;
                let items = (0..ty.fixed_length() as usize)
                    .map(|i| self.scalar(elem, pos + i * size))
                    .collect::<Result<Vec<_>>>()?;
                Ok(RawValue::Scalars(items))
            }
            _ => self.scalar_field(&ty, pos, owner, field.name()),
        }
    }

    fn scalar_field(
        &self,
        ty: &Type<'a>,
        pos: usize,
        owner: &str,
        fname: &str,
    ) -> Result<RawValue> {
        let v = self.scalar(ty.base_type(), pos)?;
        if ty.index() >= 0 {
            let value = match v {
                RawValue::Int(i) => i,
                RawValue::UInt(u) => u as i64,
                _ => return Ok(v),
            };
            let name = self
                .schema
                .enums()
                .get(ty.index() as usize)
                .values()
                .iter()
                .find(|e| e.value() == value)
                .map(|e| e.name().to_string());
            return Ok(RawValue::Enum { name, value });
        }
        Ok(self.annotator.scalar(owner, fname, v))
    }

    fn scalar(&self, bt: BaseType, pos: usize) -> Result<RawValue> {
        Ok(match bt {
            BaseType::Bool => RawValue::Bool(self.u8_at(pos)? != 0),
            BaseType::UType | BaseType::UByte => RawValue::UInt(self.u8_at(pos)? as u64),
            BaseType::Byte => RawValue::Int(self.u8_at(pos)? as i8 as i64),
            BaseType::Short => RawValue::Int(i16::from_le_bytes(self.array(pos)?) as i64),
            BaseType::UShort => RawValue::UInt(self.u16_at(pos)? as u64),
            BaseType::Int => RawValue::Int(self.i32_at(pos)? as i64),
            BaseType::UInt => RawValue::UInt(self.u32_at(pos)? as u64),
            BaseType::Long => RawValue::Int(i64::from_le_bytes(self.array(pos)?)),
            BaseType::ULong => RawValue::UInt(u64::from_le_bytes(self.array(pos)?)),
            BaseType::Float => RawValue::Float(f32::from_le_bytes(self.array(pos)?) as f64),
            BaseType::Double => RawValue::Float(f64::from_le_bytes(self.array(pos)?)),
            other => bail!("unsupported scalar type {other:?}"),
        })
    }

    fn object(&self, index: i32) -> Result<Object<'a>> {
        let objects = self.schema.objects();
        usize::try_from(index)
            .ok()
            .filter(|i| *i < objects.len())
            .map(|i| objects.get(i))
            .ok_or_else(|| eyre!("schema object index {index} out of range"))
    }

    fn follow(&self, pos: usize) -> Result<usize> {
        Ok(pos + self.u32_at(pos)? as usize)
    }

    fn string_at(&self, loc: usize) -> Result<String> {
        let len = self.u32_at(loc)? as usize;
        let bytes = self.slice(loc + 4, len)?;
        Ok(crate::sanitize::sanitize(&String::from_utf8_lossy(bytes)))
    }

    fn slice(&self, pos: usize, len: usize) -> Result<&'a [u8]> {
        pos.checked_add(len)
            .and_then(|end| self.buf.get(pos..end))
            .ok_or_else(|| {
                eyre!("read of {len} bytes at offset {pos} is past the end of the buffer")
            })
    }

    fn array<const N: usize>(&self, pos: usize) -> Result<[u8; N]> {
        Ok(self.slice(pos, N)?.try_into().expect("slice has length N"))
    }

    fn u8_at(&self, pos: usize) -> Result<u8> {
        Ok(self.array::<1>(pos)?[0])
    }

    fn u16_at(&self, pos: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array(pos)?))
    }

    fn u32_at(&self, pos: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array(pos)?))
    }

    fn i32_at(&self, pos: usize) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array(pos)?))
    }
}

/// Fields in declaration order (the schema stores them sorted by name).
fn sorted_fields<'a>(obj: &Object<'a>) -> Vec<Field<'a>> {
    let mut fields: Vec<Field> = obj.fields().iter().collect();
    fields.sort_by_key(|f| f.id());
    fields
}

fn find_field<'a>(obj: &Object<'a>, name: &str) -> Option<Field<'a>> {
    obj.fields().iter().find(|f| f.name() == name)
}

fn field_names(obj: &Object) -> Vec<String> {
    sorted_fields(obj)
        .iter()
        .filter(|f| f.type_().base_type() != BaseType::UType)
        .map(|f| f.name().to_string())
        .collect()
}

fn scalar_size(bt: BaseType) -> Option<usize> {
    Some(match bt {
        BaseType::Bool | BaseType::Byte | BaseType::UByte | BaseType::UType => 1,
        BaseType::Short | BaseType::UShort => 2,
        BaseType::Int | BaseType::UInt | BaseType::Float => 4,
        BaseType::Long | BaseType::ULong | BaseType::Double => 8,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_parse() {
        assert_eq!(
            parse_selector("arrays/0/refs/10..20").unwrap(),
            vec![
                Seg::Field("arrays".into()),
                Seg::Index(0),
                Seg::Field("refs".into()),
                Seg::Range(10, 20)
            ]
        );
        assert_eq!(
            parse_selector("/nodes/").unwrap(),
            vec![Seg::Field("nodes".into())]
        );
        assert_eq!(
            parse_selector("refs/5..").unwrap(),
            vec![Seg::Field("refs".into()), Seg::Range(5, usize::MAX)]
        );
        assert!(parse_selector("refs/9..3").is_err());
        assert!(parse_selector("refs/x..3").is_err());
    }
}
