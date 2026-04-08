//! Simple tree renderer for the sidebar.
//!
//! Flattens a hierarchical tree into a list with depth-based indentation
//! and Unicode box-drawing connectors.

use std::collections::HashSet;

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::store::{DataStore, LoadState, TreeNode};
use crate::store::types::TreeNodeType;
use crate::theme::Theme;

/// A flattened tree entry ready for rendering
pub struct FlatEntry {
    pub path: String,
    pub name: String,
    pub node_type: TreeNodeType,
    pub depth: usize,
    pub is_last: bool, // last sibling at this depth
    pub connector_prefix: String,
}

/// Tree state: which nodes are expanded, which is selected
pub struct TreeViewState {
    pub expanded: HashSet<String>,
    pub selected_index: usize,
    /// Cached flat list (rebuilt when data changes)
    pub flat_entries: Vec<FlatEntry>,
}

impl Default for TreeViewState {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            selected_index: 0,
            flat_entries: Vec::new(),
        }
    }
}

impl TreeViewState {
    pub fn move_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if !self.flat_entries.is_empty() {
            self.selected_index = (self.selected_index + 1).min(self.flat_entries.len() - 1);
        }
    }

    pub fn toggle_selected(&mut self) {
        if let Some(entry) = self.flat_entries.get(self.selected_index) {
            if matches!(entry.node_type, TreeNodeType::Group) {
                let path = entry.path.clone();
                if self.expanded.contains(&path) {
                    self.expanded.remove(&path);
                } else {
                    self.expanded.insert(path);
                }
            }
        }
    }

    /// Get the path of the currently selected entry
    pub fn selected_path(&self) -> Option<&str> {
        self.flat_entries
            .get(self.selected_index)
            .map(|e| e.path.as_str())
    }

    /// Rebuild the flat list from store data
    pub fn rebuild(&mut self, store: &DataStore) {
        self.flat_entries.clear();
        if let Some(LoadState::Loaded(nodes)) = store.node_children.get("/") {
            let count = nodes.len();
            for (i, node) in nodes.iter().enumerate() {
                self.flatten_node(node, store, 0, i == count - 1, String::new());
            }
        }
        // Clamp selection
        if !self.flat_entries.is_empty() {
            self.selected_index = self.selected_index.min(self.flat_entries.len() - 1);
        }
    }

    fn flatten_node(
        &mut self,
        node: &TreeNode,
        store: &DataStore,
        depth: usize,
        is_last: bool,
        parent_prefix: String,
    ) {
        let connector = if depth == 0 {
            String::new()
        } else if is_last {
            format!("{}└─", parent_prefix)
        } else {
            format!("{}├─", parent_prefix)
        };

        self.flat_entries.push(FlatEntry {
            path: node.path.clone(),
            name: node.name.clone(),
            node_type: node.node_type.clone(),
            depth,
            is_last,
            connector_prefix: connector,
        });

        // If group is expanded and children are loaded, recurse
        if matches!(node.node_type, TreeNodeType::Group)
            && self.expanded.contains(&node.path)
        {
            if let Some(LoadState::Loaded(children)) = store.node_children.get(&node.path) {
                let child_prefix = if depth == 0 {
                    String::new()
                } else if is_last {
                    format!("{}  ", parent_prefix)
                } else {
                    format!("{}│ ", parent_prefix)
                };

                let count = children.len();
                for (i, child) in children.iter().enumerate() {
                    self.flatten_node(child, store, depth + 1, i == count - 1, child_prefix.clone());
                }
            }
        }
    }
}

/// Render the tree into the given area
pub fn render_tree(
    state: &TreeViewState,
    theme: &Theme,
    focused: bool,
    frame: &mut Frame,
    area: Rect,
) {
    if state.flat_entries.is_empty() {
        frame.render_widget(
            Paragraph::new("  (empty)").style(theme.text_dim),
            area,
        );
        return;
    }

    // Calculate visible range based on selection and area height
    let height = area.height as usize;
    let offset = if state.selected_index >= height {
        state.selected_index - height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = state
        .flat_entries
        .iter()
        .skip(offset)
        .take(height)
        .enumerate()
        .map(|(visible_i, entry)| {
            let actual_i = offset + visible_i;
            let is_selected = focused && actual_i == state.selected_index;

            let (icon, icon_style) = match &entry.node_type {
                TreeNodeType::Group => {
                    if state.expanded.contains(&entry.path) {
                        ("▼ ", theme.group_icon)
                    } else {
                        ("▶ ", theme.group_icon)
                    }
                }
                TreeNodeType::Array(_) => ("─ ", theme.array_icon),
            };

            let detail = match &entry.node_type {
                TreeNodeType::Array(summary) => {
                    let shape = summary
                        .shape
                        .iter()
                        .map(|d| d.to_string())
                        .collect::<Vec<_>>()
                        .join("×");
                    format!(" [{}]", shape)
                }
                _ => String::new(),
            };

            let name_style = if is_selected {
                theme.selected
            } else {
                theme.text
            };

            let line = Line::from(vec![
                Span::styled(&entry.connector_prefix, theme.text_dim),
                Span::styled(icon, icon_style),
                Span::styled(&entry.name, name_style),
                Span::styled(detail, theme.text_dim),
            ]);
            ListItem::new(line)
        })
        .collect();

    frame.render_widget(List::new(items), area);
}
