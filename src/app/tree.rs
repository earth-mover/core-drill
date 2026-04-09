use crate::store::{LoadState, TreeNodeType};

use super::App;

impl App {
    /// Build the full identifier path for tui_tree_widget selection.
    /// For "/stations/latitude" returns ["/stations", "/stations/latitude"].
    pub(crate) fn tree_identifier_path(path: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut built = String::new();
        for seg in segments {
            built = format!("{}/{}", built, seg);
            parts.push(built.clone());
        }
        parts
    }

    /// Select a tree node by path, opening all ancestor groups.
    pub(crate) fn select_tree_node(&mut self, path: &str) {
        let id_path = Self::tree_identifier_path(path);
        // Open all ancestor groups so the node is visible
        for i in 0..id_path.len().saturating_sub(1) {
            self.tree_state.open(id_path[..=i].to_vec());
        }
        self.tree_state.select(id_path);
    }

    /// Open the selected node and all its descendants (zO).
    pub(crate) fn open_tree_deep(&mut self) {
        let selected = self.tree_state.selected().to_vec();
        if selected.is_empty() {
            return;
        }
        // Open the selected node itself
        self.tree_state.open(selected.clone());
        // Open all descendants by finding paths that start with the selected path
        let selected_path = selected.last().cloned().unwrap_or_default();
        let prefix = format!("{selected_path}/");
        let group_paths: Vec<String> = self.store.node_children.keys()
            .filter(|p| p.starts_with(&prefix) || **p == selected_path)
            .cloned()
            .collect();
        for path in group_paths {
            let id_path = Self::tree_identifier_path(&path);
            self.tree_state.open(id_path);
        }
    }

    /// Close the selected node and all its descendants (zC).
    /// If the selected node is a leaf or already closed, bubbles up to parent (like zc).
    pub(crate) fn close_tree_deep(&mut self) {
        let selected = self.tree_state.selected().to_vec();
        if selected.is_empty() {
            return;
        }
        let selected_path = selected.last().cloned().unwrap_or_default();
        let prefix = format!("{selected_path}/");

        // Check if this node has any descendants to close
        let has_descendants = self.store.node_children.keys()
            .any(|p| p.starts_with(&prefix));

        if has_descendants {
            // Close all descendants
            let group_paths: Vec<Vec<String>> = self.store.node_children.keys()
                .filter(|p| p.starts_with(&prefix))
                .map(|p| Self::tree_identifier_path(p))
                .collect();
            for id_path in &group_paths {
                self.tree_state.close(id_path);
            }
            self.tree_state.close(&selected);
        } else if selected.len() > 1 {
            // Leaf or already closed — bubble up to parent (like zc)
            let parent = selected[..selected.len() - 1].to_vec();
            // Close parent and all its descendants
            let parent_path = parent.last().cloned().unwrap_or_default();
            let parent_prefix = format!("{parent_path}/");
            let group_paths: Vec<Vec<String>> = self.store.node_children.keys()
                .filter(|p| p.starts_with(&parent_prefix))
                .map(|p| Self::tree_identifier_path(p))
                .collect();
            for id_path in &group_paths {
                self.tree_state.close(id_path);
            }
            self.tree_state.close(&parent);
            self.tree_state.select(parent);
            self.on_tree_selection_changed();
        }
    }

    /// Open all nodes in the entire tree (zR).
    pub(crate) fn open_all_tree_nodes(&mut self) {
        let group_paths: Vec<Vec<String>> = self.store.node_children.keys()
            .map(|p| Self::tree_identifier_path(p))
            .collect();
        for id_path in group_paths {
            self.tree_state.open(id_path);
        }
    }

    /// Auto-expand the tree when root children are all groups.
    /// Drills down through single-child groups so the user lands on
    /// the first meaningful level of the hierarchy.
    pub(crate) fn auto_expand_tree(&mut self) {
        let mut current_path = "/".to_string();
        let mut identifier_path: Vec<String> = Vec::new();

        while let Some(LoadState::Loaded(children)) = self.store.node_children.get(&current_path) {
            if children.is_empty() {
                break;
            }

            let all_groups = children
                .iter()
                .all(|n| matches!(n.node_type, TreeNodeType::Group));

            if all_groups && children.len() == 1 {
                // Single group child — open it and keep drilling
                let child = &children[0];
                identifier_path.push(child.path.clone());
                self.tree_state.open(identifier_path.clone());
                current_path = child.path.clone();
            } else if all_groups {
                // Multiple group children — open them all, then select the first
                for child in children {
                    let mut id = identifier_path.clone();
                    id.push(child.path.clone());
                    self.tree_state.open(id);
                }
                // Select the first child
                let first = &children[0];
                let mut select_id = identifier_path.clone();
                select_id.push(first.path.clone());
                self.tree_state.select(select_id);
                return;
            } else {
                // Mixed or all arrays — select the first leaf (array) or first node
                if let Some(first_leaf) = children
                    .iter()
                    .find(|n| matches!(n.node_type, TreeNodeType::Array(_)))
                {
                    let mut select_id = identifier_path.clone();
                    select_id.push(first_leaf.path.clone());
                    self.tree_state.select(select_id);
                } else {
                    let mut select_id = identifier_path.clone();
                    select_id.push(children[0].path.clone());
                    self.tree_state.select(select_id);
                }
                return;
            }
        }

        // Fallback: just select the first visible node
        self.tree_state.select_first();
    }
}
