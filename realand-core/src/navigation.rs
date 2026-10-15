//! Stable desktop-space topology and display selection without device payloads.
//!
//! Selected output/displayed spaces are separate from the nullable global
//! keyboard-focus scope: an overview can suspend keyboard focus while retaining
//! the displayed desktop. A physical adapter translates these IDs to its slots.

use std::collections::HashMap;

use crate::SpaceId;

#[derive(Debug, Clone, Default)]
pub struct OutputSpaces {
    connected: bool,
    spaces: Vec<SpaceId>,
    active: Option<SpaceId>,
    previous: Option<SpaceId>,
    remembered: Option<SpaceId>,
}

impl OutputSpaces {
    pub fn spaces(&self) -> &[SpaceId] {
        &self.spaces
    }
    pub fn active(&self) -> Option<SpaceId> {
        self.active
    }
    pub fn previous(&self) -> Option<SpaceId> {
        self.previous
    }
    pub fn connected(&self) -> bool {
        self.connected
    }

    fn remove(&mut self, id: SpaceId) -> bool {
        let Some(index) = self.spaces.iter().position(|space| *space == id) else {
            return false;
        };
        self.spaces.remove(index);
        if self.active == Some(id) {
            // Workspace removal intentionally prefers the preceding desktop.
            self.active = self.spaces.get(index.saturating_sub(1)).copied();
        }
        true
    }
}

#[derive(Debug, Clone, Default)]
pub struct OutputSpacePolicy {
    outputs: HashMap<String, OutputSpaces>,
    connected_order: Vec<String>,
    owners: HashMap<SpaceId, String>,
    detached: Vec<SpaceId>,
    selected_output: Option<String>,
}

impl OutputSpacePolicy {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn output(&self, connector: &str) -> Option<&OutputSpaces> {
        self.outputs.get(connector)
    }
    pub fn output_order(&self) -> &[String] {
        &self.connected_order
    }
    /// Ordered live spaces without an output association. These include a session
    /// with no outputs and temporary transfers; attaching removes the same ID.
    pub fn detached_spaces(&self) -> &[SpaceId] {
        &self.detached
    }

    pub fn register_space(&mut self, id: SpaceId) -> bool {
        if self.owners.contains_key(&id) || self.detached.contains(&id) {
            return false;
        }
        self.detached.push(id);
        true
    }

    pub fn reorder_detached_space(&mut self, id: SpaceId, index: usize) -> bool {
        let Some(old) = self.detached.iter().position(|space| *space == id) else {
            return false;
        };
        let index = index.min(self.detached.len() - 1);
        if index == old {
            return false;
        }
        self.detached.remove(old);
        self.detached.insert(index, id);
        true
    }

    pub fn selected_output(&self) -> Option<&str> {
        self.selected_output.as_deref()
    }
    pub fn owner(&self, id: SpaceId) -> Option<&str> {
        self.owners.get(&id).map(String::as_str)
    }
    pub fn active_space(&self, connector: &str) -> Option<SpaceId> {
        self.output(connector)?.active
    }
    pub fn selected_space(&self) -> Option<SpaceId> {
        self.active_space(self.selected_output()?)
    }
    pub fn previous_space(&self, connector: &str) -> Option<SpaceId> {
        self.output(connector)?.previous
    }

    pub fn connect_output(&mut self, connector: String) -> bool {
        let output = self.outputs.entry(connector.clone()).or_default();
        if output.connected {
            return false;
        }
        output.connected = true;
        self.connected_order.push(connector.clone());
        if self.selected_output.is_none() {
            self.selected_output = Some(connector);
        }
        true
    }

    pub fn disconnect_output(&mut self, connector: &str) -> bool {
        let Some(output) = self.outputs.get_mut(connector) else {
            return false;
        };
        if !output.connected {
            return false;
        }
        output.connected = false;
        output.remembered = output.active;
        let old_index = self
            .connected_order
            .iter()
            .position(|name| name == connector)
            .unwrap();
        self.connected_order.remove(old_index);
        if self.selected_output.as_deref() == Some(connector) {
            self.selected_output = self
                .connected_order
                .get(old_index.saturating_sub(1))
                .cloned();
        }
        // Keep its stable space identities until explicit transfer/detach events.
        true
    }

    pub fn select_output(&mut self, connector: &str) -> bool {
        if !self
            .outputs
            .get(connector)
            .is_some_and(|output| output.connected)
        {
            return false;
        }
        if self.selected_output.as_deref() == Some(connector) {
            return false;
        }
        self.selected_output = Some(connector.to_owned());
        true
    }

    pub fn reorder_output(&mut self, connector: &str, index: usize) -> bool {
        let Some(previous) = self
            .connected_order
            .iter()
            .position(|name| name == connector)
        else {
            return false;
        };
        let index = index.min(self.connected_order.len() - 1);
        if index == previous {
            return false;
        }
        let connector = self.connected_order.remove(previous);
        self.connected_order.insert(index, connector);
        true
    }

    /// Attach, transfer or reorder a space at its final destination index.
    /// Nonactive insertion preserves active identity; no index repair is needed.
    pub fn attach_space(&mut self, connector: &str, id: SpaceId, index: usize) -> bool {
        if !self.outputs.contains_key(connector) {
            return false;
        }
        if self.owner(id) == Some(connector) {
            return self.reorder_space(id, index);
        }
        self.detach_space(id);
        self.detached.retain(|space| *space != id);
        let output = self.outputs.get_mut(connector).unwrap();
        output.spaces.insert(index.min(output.spaces.len()), id);
        if output.active.is_none() {
            output.active = Some(id);
        }
        self.owners.insert(id, connector.to_owned());
        true
    }

    pub fn detach_space(&mut self, id: SpaceId) -> bool {
        let Some(owner) = self.owners.remove(&id) else {
            return false;
        };
        self.outputs.get_mut(&owner).unwrap().remove(id);
        self.register_space(id);
        true
    }

    pub fn forget_space(&mut self, id: SpaceId) {
        self.detach_space(id);
        self.detached.retain(|space| *space != id);
        for output in self.outputs.values_mut() {
            if output.previous == Some(id) {
                output.previous = None;
            }
            if output.remembered == Some(id) {
                output.remembered = None;
            }
        }
    }

    pub fn reorder_space(&mut self, id: SpaceId, index: usize) -> bool {
        let Some(owner) = self.owners.get(&id) else {
            return false;
        };
        let output = self.outputs.get_mut(owner).unwrap();
        let previous = output.spaces.iter().position(|space| *space == id).unwrap();
        let index = index.min(output.spaces.len() - 1);
        if previous == index {
            return false;
        }
        output.spaces.remove(previous);
        output.spaces.insert(index, id);
        true
    }

    pub fn select_space(&mut self, connector: &str, id: SpaceId, remember_previous: bool) -> bool {
        let Some(output) = self.outputs.get_mut(connector) else {
            return false;
        };
        if !output.spaces.contains(&id) || output.active == Some(id) {
            return false;
        }
        if remember_previous {
            output.previous = output.active;
        }
        output.active = Some(id);
        true
    }

    /// Restore the previous-ID preference after an order-only operation.
    pub fn set_previous_space(&mut self, connector: &str, previous: Option<SpaceId>) {
        if let Some(output) = self.outputs.get_mut(connector) {
            output.previous = previous;
        }
    }

    pub fn remember_active_space(&mut self, connector: &str) {
        if let Some(output) = self.outputs.get_mut(connector) {
            if output.active.is_some() {
                output.remembered = output.active;
            }
        }
    }

    pub fn take_remembered_space(&mut self, connector: &str) -> Option<SpaceId> {
        self.outputs.get_mut(connector)?.remembered.take()
    }

    pub fn verify_invariants(&self) -> bool {
        let connected: std::collections::HashSet<_> = self.connected_order.iter().collect();
        if connected.len() != self.connected_order.len()
            || self
                .outputs
                .iter()
                .any(|(name, output)| output.connected != connected.contains(name))
            || self
                .selected_output
                .as_ref()
                .is_some_and(|name| !connected.contains(name))
            || self.selected_output.is_none() != self.connected_order.is_empty()
        {
            return false;
        }
        let mut spaces = std::collections::HashSet::new();
        for (name, output) in &self.outputs {
            if output.active.is_none() != output.spaces.is_empty()
                || output.active.is_some_and(|id| !output.spaces.contains(&id))
            {
                return false;
            }
            for id in &output.spaces {
                if !spaces.insert(*id) || self.owner(*id) != Some(name.as_str()) {
                    return false;
                }
            }
        }
        if spaces.len() != self.owners.len() {
            return false;
        }
        for id in &self.detached {
            if !spaces.insert(*id) || self.owners.contains_key(id) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_identity_survives_insertion_reordering_and_removal_of_other_spaces() {
        let mut policy = OutputSpacePolicy::new();
        policy.connect_output("DP-1".into());
        for id in 1..=4 {
            policy.attach_space("DP-1", SpaceId(id), usize::MAX);
        }
        policy.select_space("DP-1", SpaceId(3), true);
        policy.attach_space("DP-1", SpaceId(5), 0);
        assert_eq!(policy.active_space("DP-1"), Some(SpaceId(3)));
        policy.reorder_space(SpaceId(3), 0);
        policy.detach_space(SpaceId(1));
        assert_eq!(policy.active_space("DP-1"), Some(SpaceId(3)));
        assert_eq!(policy.previous_space("DP-1"), Some(SpaceId(1)));
        policy.forget_space(SpaceId(1));
        assert_eq!(policy.previous_space("DP-1"), None);
        assert!(policy.verify_invariants());
    }

    #[test]
    fn removing_current_space_prefers_previous_and_transfer_never_duplicates_identity() {
        let mut policy = OutputSpacePolicy::new();
        policy.connect_output("DP-1".into());
        policy.connect_output("DP-2".into());
        for id in 1..=3 {
            policy.attach_space("DP-1", SpaceId(id), usize::MAX);
        }
        policy.select_space("DP-1", SpaceId(2), true);
        policy.attach_space("DP-2", SpaceId(2), 0);
        assert_eq!(policy.active_space("DP-1"), Some(SpaceId(1)));
        assert_eq!(policy.active_space("DP-2"), Some(SpaceId(2)));
        assert_eq!(policy.owner(SpaceId(2)), Some("DP-2"));
        assert_eq!(policy.selected_output(), Some("DP-1"));
        assert!(policy.verify_invariants());
    }

    #[test]
    fn disconnected_selection_and_reconnect_memory_keep_empty_space_ids() {
        let mut policy = OutputSpacePolicy::new();
        policy.connect_output("DP-1".into());
        policy.connect_output("DP-2".into());
        policy.attach_space("DP-1", SpaceId(10), 0);
        policy.attach_space("DP-2", SpaceId(20), 0);
        policy.select_output("DP-2");
        policy.disconnect_output("DP-2");
        assert_eq!(policy.selected_output(), Some("DP-1"));
        policy.detach_space(SpaceId(20));
        policy.connect_output("DP-2".into());
        assert_eq!(policy.take_remembered_space("DP-2"), Some(SpaceId(20)));
        policy.attach_space("DP-2", SpaceId(20), 0);
        assert_eq!(policy.active_space("DP-2"), Some(SpaceId(20)));
        assert_eq!(policy.take_remembered_space("DP-2"), None);
        assert!(policy.verify_invariants());
    }
}
