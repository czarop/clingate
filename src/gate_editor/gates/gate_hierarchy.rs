use anyhow::{Result, anyhow};
use rustc_hash::FxHashMap;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

/// Manages the hierarchical relationships between gates.
///
/// Gate hierarchies represent parent-child relationships where child gates
/// are applied to events that pass through their parent gates. This enables
/// sequential gating strategies common in flow cytometry analysis.
///
/// # Relationship to `flow_gates::GateHierarchy`
///
/// This is a deliberate fork of `flow_gates::hierarchy::GateHierarchy`, kept
/// separate rather than merged back. The difference is sibling ordering: this
/// version carries an `orders` map and threads a `u64` order through
/// [`add_child`](Self::add_child) and [`add_gate_child`](Self::add_gate_child),
/// so children sort by Omiq's `ord` field and an imported gating tree renders in
/// the order Omiq showed it. The flow-gates version takes no order argument and
/// leaves siblings in insertion order.
///
/// Keep the two in step by hand when fixing a bug in shared logic. The tests at
/// the bottom of this file cover this version's behaviour and are the spec if
/// the two are ever reconciled.
///
/// The hierarchy is represented as a directed acyclic graph (DAG), preventing
/// cycles while allowing multiple parents per child (though this implementation
/// currently supports single-parent hierarchies).
///
/// # Example
///
/// ```rust
/// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
///
/// let mut hierarchy = GateHierarchy::new();
///
/// // Build hierarchy: root -> parent -> child
/// hierarchy.add_child("root", "parent", 0);
/// hierarchy.add_child("parent", "child", 0);
///
/// // Get ancestors
/// let ancestors = hierarchy.get_ancestors("child");
/// assert_eq!(ancestors.len(), 2);
///
/// // Get chain from root to child
/// let chain = hierarchy.get_chain_to_root("child");
/// assert_eq!(chain.len(), 3);
///
/// // Prevent cycles
/// assert!(!hierarchy.add_child("child", "root", 0)); // Would create cycle
/// ```
#[derive(Debug, Clone, Default)]
pub struct GateHierarchy {
    /// Maps parent gate ID to list of child gate IDs
    children: FxHashMap<Arc<str>, Vec<Arc<str>>>,
    /// Maps child gate ID to parent gate ID
    parents: FxHashMap<Arc<str>, Arc<str>>,

    orders: FxHashMap<Arc<str>, u64>,
}

impl GateHierarchy {
    /// Create a new empty hierarchy
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a root gate (gate with no parent)
    ///
    /// Ensures a gate is registered in the hierarchy as a root node.
    /// This is necessary for gates that don't have parents to appear in get_roots().
    pub fn add_root(&mut self, gate_id: impl Into<Arc<str>>) {
        let gate_id = gate_id.into();
        // Ensure the gate exists in the children map with an empty list
        // This makes it appear in get_roots() without having a parent
        self.children.entry(gate_id.clone()).or_default();
        self.orders.entry(gate_id).or_insert(0);
    }

    /// Add a child-parent relationship
    ///
    /// Returns `true` if the relationship was added, `false` if it would create a cycle
    pub fn add_child(
        &mut self,
        parent_id: impl Into<Arc<str>>,
        child_id: impl Into<Arc<str>>,
        order: u64,
    ) -> bool {
        let parent_id = parent_id.into();
        let child_id = child_id.into();

        // Check for cycles before adding
        if self.would_create_cycle(&parent_id, &child_id) {
            return false;
        }

        self.orders.insert(child_id.clone(), order);

        // Remove child from previous parent if it exists
        if let Some(old_parent) = self.parents.get(&child_id)
            && let Some(siblings) = self.children.get_mut(old_parent)
        {
            siblings.retain(|id| id != &child_id);
        }

        // Add new relationship
        let siblings = self.children.entry(parent_id.clone()).or_default();

        siblings.push(child_id.clone());

        let orders = &self.orders;
        siblings.sort_by_key(|id| orders.get(id).cloned().unwrap_or(0));

        self.parents.insert(child_id, parent_id);

        true
    }

    /// Remove a gate and all its relationships
    ///
    /// Children of the removed gate become orphans (no parent)
    pub fn remove_node(&mut self, gate_id: &str) {
        // Remove as a child
        if let Some(parent_id) = self.parents.remove(gate_id)
            && let Some(siblings) = self.children.get_mut(&parent_id)
        {
            siblings.retain(|id| id.as_ref() != gate_id);
        }

        // Remove as a parent (orphan the children)
        if let Some(child_ids) = self.children.remove(gate_id) {
            for child_id in child_ids {
                self.parents.remove(&child_id);
            }
        }
    }

    /// Remove a parent-child relationship
    pub fn remove_child(&mut self, parent_id: &str, child_id: &str) {
        if let Some(children) = self.children.get_mut(parent_id) {
            children.retain(|id| id.as_ref() != child_id);
        }

        if self.parents.get(child_id).map(|p| p.as_ref()) == Some(parent_id) {
            self.parents.remove(child_id);
        }
    }

    /// Get the parent of a gate
    pub fn get_parent(&self, gate_id: &str) -> Option<&Arc<str>> {
        self.parents.get(gate_id)
    }

    /// Get the children of a gate
    pub fn get_children(&self, gate_id: &str) -> Vec<&Arc<str>> {
        self.children
            .get(gate_id)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Get all ancestors of a gate (parent, grandparent, etc.) in order from closest to root
    pub fn get_ancestors(&self, gate_id: &str) -> Vec<Arc<str>> {
        let mut ancestors = Vec::new();
        let mut current = gate_id;

        while let Some(parent) = self.parents.get(current) {
            ancestors.push(parent.clone());
            current = parent.as_ref();
        }

        ancestors
    }

    /// Get all descendants of a gate (children, grandchildren, etc.)
    pub fn get_descendants(&self, gate_id: &str) -> Vec<Arc<str>> {
        let mut descendants = Vec::new();
        let mut queue = VecDeque::new();

        if let Some(children) = self.children.get(gate_id) {
            for child in children {
                queue.push_back(child.clone());
            }
        }

        while let Some(node) = queue.pop_front() {
            descendants.push(node.clone());

            if let Some(children) = self.children.get(&node) {
                for child in children {
                    queue.push_back(child.clone());
                }
            }
        }

        descendants
    }

    /// Get the full chain from root to this gate (including the gate itself)
    pub fn get_chain_to_root(&self, gate_id: &str) -> Vec<Arc<str>> {
        let mut chain = self.get_ancestors(gate_id);
        chain.reverse(); // Root first
        chain.push(Arc::from(gate_id));
        chain
    }

    /// Get all root gates (gates with no parents)
    pub fn get_roots(&self) -> Vec<Arc<str>> {
        let all_gates: HashSet<_> = self.children.keys().chain(self.parents.keys()).collect();

        all_gates
            .into_iter()
            .filter(|gate_id| !self.parents.contains_key(*gate_id))
            .cloned()
            .collect()
    }

    /// Perform a topological sort of the gates
    ///
    /// Returns gates in an order where parents come before children
    /// Returns None if there are cycles
    pub fn topological_sort(&self) -> Option<Vec<Arc<str>>> {
        let mut result = Vec::new();
        let mut in_degree: HashMap<Arc<str>, usize> = HashMap::new();
        let mut queue = VecDeque::new();

        // Collect all gates
        let all_gates: HashSet<Arc<str>> = self
            .children
            .keys()
            .chain(self.parents.keys())
            .cloned()
            .collect();

        // Calculate in-degrees
        for gate in &all_gates {
            in_degree.insert(gate.clone(), 0);
        }

        for children in self.children.values() {
            for child in children {
                *in_degree.entry(child.clone()).or_insert(0) += 1;
            }
        }

        // Find gates with no incoming edges (roots)
        for (gate, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(gate.clone());
            }
        }

        // Process queue
        while let Some(gate) = queue.pop_front() {
            result.push(gate.clone());

            if let Some(children) = self.children.get(&gate) {
                for child in children {
                    if let Some(degree) = in_degree.get_mut(child) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(child.clone());
                        }
                    }
                }
            }
        }

        // Check if all gates were processed (no cycles)
        if result.len() == all_gates.len() {
            Some(result)
        } else {
            None // Cycle detected
        }
    }

    /// Check if adding a parent-child relationship would create a cycle
    fn would_create_cycle(&self, parent_id: &Arc<str>, child_id: &Arc<str>) -> bool {
        // If parent is already a descendant of child, adding this edge would create a cycle
        let descendants = self.get_descendants(child_id.as_ref());
        descendants.contains(parent_id)
    }

    /// This gate's sort order among its siblings.
    ///
    /// Imported gates carry Omiq's own `ord`; gates created here are given a
    /// millisecond timestamp, which is the same shape Omiq uses.
    pub fn get_order(&self, gate_id: &str) -> Option<u64> {
        self.orders.get(gate_id).copied()
    }

    /// Get the depth of a gate in the hierarchy (root = 0)
    pub fn get_depth(&self, gate_id: &str) -> usize {
        self.get_ancestors(gate_id).len()
    }

    /// Check if a gate is a root (has no parent)
    pub fn is_root(&self, gate_id: &str) -> bool {
        !self.parents.contains_key(gate_id)
    }

    /// Check if a gate is a leaf (has no children)
    pub fn is_leaf(&self, gate_id: &str) -> bool {
        self.children
            .get(gate_id)
            .map(|c| c.is_empty())
            .unwrap_or(true)
    }

    /// Get all leaf gates (gates with no children)
    pub fn get_leaves(&self) -> Vec<Arc<str>> {
        let all_gates: HashSet<Arc<str>> = self
            .children
            .keys()
            .chain(self.parents.keys())
            .cloned()
            .collect();

        all_gates
            .into_iter()
            .filter(|gate_id| self.is_leaf(gate_id.as_ref()))
            .collect()
    }

    /// Clear all relationships
    pub fn clear(&mut self) {
        self.children.clear();
        self.parents.clear();
    }

    /// Reparent a single gate to a new parent
    ///
    /// Moves a gate from its current parent to a new parent. This is equivalent
    /// to removing the gate from its current parent and adding it to the new parent.
    ///
    /// # Arguments
    /// * `gate_id` - The ID of the gate to reparent
    /// * `new_parent_id` - The ID of the new parent gate
    ///
    /// # Returns
    /// `Ok(())` if successful, or an error if:
    /// - The gate doesn't exist
    /// - The new parent doesn't exist
    /// - The operation would create a cycle
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("parent1", "child", 0);
    /// hierarchy.add_root("parent2");
    /// hierarchy.reparent("child", "parent2")?;
    /// assert_eq!(hierarchy.get_parent("child").map(|s| s.as_ref()), Some("parent2"));
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn reparent(
        &mut self,
        gate_id: impl Into<Arc<str>>,
        new_parent_id: impl Into<Arc<str>>,
    ) -> Result<()> {
        let gate_id = gate_id.into();
        let new_parent_id = new_parent_id.into();

        // Check if gate exists (it might be a root)
        let all_gates: HashSet<Arc<str>> = self
            .children
            .keys()
            .chain(self.parents.keys())
            .cloned()
            .collect();

        if !all_gates.contains(&gate_id) {
            return Err(anyhow!("gate not found in hierarchy {}", gate_id.as_ref()));
        }

        if !all_gates.contains(&new_parent_id) && gate_id != new_parent_id {
            return Err(anyhow!(
                "Parent gate not found in hierarchy {}",
                new_parent_id.as_ref(),
            ));
        }

        // Check for cycles
        if self.would_create_cycle(&new_parent_id, &gate_id) {
            return Err(anyhow!(
                "Would create cycle {} {}",
                new_parent_id.as_ref(),
                gate_id.as_ref(),
            ));
        }

        // Remove from current parent if it exists
        if let Some(old_parent) = self.parents.remove(&gate_id)
            && let Some(siblings) = self.children.get_mut(&old_parent)
        {
            siblings.retain(|id| id != &gate_id);
        }

        // Add to new parent
        self.children
            .entry(new_parent_id.clone())
            .or_default()
            .push(gate_id.clone());
        self.parents.insert(gate_id, new_parent_id);

        Ok(())
    }

    /// Reparent a gate and all its descendants to a new parent
    ///
    /// Moves an entire subtree (gate and all its descendants) to a new parent.
    /// This is useful for reorganizing large portions of the hierarchy.
    ///
    /// # Arguments
    /// * `gate_id` - The ID of the root gate of the subtree to move
    /// * `new_parent_id` - The ID of the new parent gate
    ///
    /// # Returns
    /// `Ok(())` if successful, or an error if the operation would create a cycle
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("parent1", "child", 0);
    /// hierarchy.add_child("child", "grandchild", 0);
    /// hierarchy.add_root("parent2");
    /// hierarchy.reparent_subtree("child", "parent2")?;
    /// // "child" is now under "parent2", and "grandchild" still under it
    /// assert_eq!(hierarchy.get_parent("child").map(|s| s.as_ref()), Some("parent2"));
    /// assert_eq!(hierarchy.get_parent("grandchild").map(|s| s.as_ref()), Some("child"));
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn reparent_subtree(
        &mut self,
        gate_id: impl Into<Arc<str>>,
        new_parent_id: impl Into<Arc<str>>,
    ) -> Result<()> {
        let gate_id = gate_id.into();
        let new_parent_id = new_parent_id.into();

        // Get all descendants
        let descendants = self.get_descendants(gate_id.as_ref());

        // Check if new_parent_id is a descendant (would create cycle)
        if descendants.contains(&new_parent_id) {
            return Err(anyhow!(
                "Would create cycle {} {}",
                new_parent_id.as_ref(),
                gate_id.as_ref(),
            ));
        }

        // Check if gate_id is a descendant of new_parent_id (would create cycle)
        let new_parent_descendants = self.get_descendants(new_parent_id.as_ref());
        if new_parent_descendants.contains(&gate_id) {
            return Err(anyhow!(
                "Would create cycle {} {}",
                new_parent_id.as_ref(),
                gate_id.as_ref(),
            ));
        }

        // Reparent the root gate
        self.reparent(gate_id.as_ref(), new_parent_id.as_ref())?;

        Ok(())
    }

    /// Clone a subtree with new IDs
    ///
    /// Creates a copy of a subtree (gate and all its descendants) with new IDs
    /// generated by the provided mapper function. The cloned subtree is returned
    /// as a new hierarchy.
    ///
    /// # Arguments
    /// * `gate_id` - The ID of the root gate of the subtree to clone
    /// * `id_mapper` - Function that maps old IDs to new IDs
    ///
    /// # Returns
    /// A new `GateHierarchy` containing the cloned subtree, or an error if the gate doesn't exist
    ///
    /// # Example
    /// ```rust,ignore
    /// // Fails today: see B-HIER-1 in docs/test-audit.md.
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("parent", "child", 0);
    /// hierarchy.add_child("child", "grandchild", 0);
    ///
    /// let cloned = hierarchy.clone_subtree("child", |id| format!("{}_copy", id))?;
    /// // cloned contains "child_copy" -> "grandchild_copy"
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn clone_subtree<F>(&self, gate_id: &str, id_mapper: F) -> Result<Self>
    where
        F: Fn(&str) -> String,
    {
        let mut new_hierarchy = Self::new();

        // Get all nodes in subtree (including root)
        let mut subtree_nodes = vec![Arc::from(gate_id)];
        subtree_nodes.extend(self.get_descendants(gate_id));

        // Map old IDs to new IDs
        let id_map: HashMap<Arc<str>, Arc<str>> = subtree_nodes
            .iter()
            .map(|old_id| {
                let new_id = Arc::from(id_mapper(old_id.as_ref()).as_str());
                (old_id.clone(), new_id)
            })
            .collect();

        // Clone relationships
        for old_id in &subtree_nodes {
            if let Some(children) = self.children.get(old_id) {
                let new_parent_id = id_map.get(old_id).unwrap();
                for child in children {
                    if let Some(new_child_id) = id_map.get(child)
                        && let Some(ord) = self.orders.get(child)
                    {
                        if !new_hierarchy.add_child(
                            new_parent_id.clone(),
                            new_child_id.clone(),
                            *ord,
                        ) {
                            return Err(anyhow!(
                                "Failed to add child in cloned hierarchy - possible cycle",
                            ));
                        } else {
                            return Err(anyhow!(
                                "Failed to add child in cloned hierarchy - no order for child {}",
                                child
                            ));
                        }
                    }
                }
            }
        }

        Ok(new_hierarchy)
    }

    /// Move an entire subtree to a new parent
    ///
    /// This is an alias for `reparent_subtree` that makes the intent clearer.
    ///
    /// # Arguments
    /// * `gate_id` - The ID of the root gate of the subtree to move
    /// * `new_parent_id` - The ID of the new parent gate
    ///
    /// # Returns
    /// `Ok(())` if successful, or an error if the operation would create a cycle
    pub fn move_subtree(
        &mut self,
        gate_id: impl Into<Arc<str>>,
        new_parent_id: impl Into<Arc<str>>,
    ) -> Result<()> {
        self.reparent_subtree(gate_id, new_parent_id)
    }

    /// Delete a gate and all its descendants
    ///
    /// Removes a gate and all of its descendants from the hierarchy.
    /// Returns a list of all deleted gate IDs.
    ///
    /// # Arguments
    /// * `gate_id` - The ID of the gate to delete (along with all descendants)
    ///
    /// # Returns
    /// A vector of all deleted gate IDs (including the root gate and all descendants)
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("parent", "child", 0);
    /// hierarchy.add_child("child", "grandchild", 0);
    ///
    /// let deleted = hierarchy.delete_subtree("child");
    /// assert_eq!(deleted.len(), 2); // "child" and "grandchild"
    /// assert!(hierarchy.get_parent("child").is_none());
    /// ```
    pub fn delete_subtree(&mut self, gate_id: &str) -> Vec<Arc<str>> {
        let mut deleted = Vec::new();
        let mut to_delete = vec![Arc::from(gate_id)];

        // Collect all descendants
        to_delete.extend(self.get_descendants(gate_id));

        // Delete all nodes
        for id in &to_delete {
            self.remove_node(id.as_ref());
            deleted.push(id.clone());
        }

        deleted
    }

    /// Delete a gate but keep its children (reparent them)
    ///
    /// Removes a gate from the hierarchy but reparents all its children to
    /// a new parent (or makes them root nodes if no parent is specified).
    ///
    /// # Arguments
    /// * `gate_id` - The ID of the gate to delete
    /// * `new_parent_id` - Optional new parent for the children. If `None`, children become root nodes
    ///
    /// # Returns
    /// A vector of reparented child IDs, or an error if:
    /// - The gate doesn't exist
    /// - The new parent doesn't exist
    /// - Reparenting would create a cycle
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("parent", "child", 0);
    /// hierarchy.add_child("child", "grandchild1", 0);
    /// hierarchy.add_child("child", "grandchild2", 0);
    ///
    /// let reparented = hierarchy.delete_node_keep_children("child", Some(Arc::from("parent")))?;
    /// assert_eq!(reparented.len(), 2);
    /// // grandchild1 and grandchild2 are now direct children of "parent"
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn delete_node_keep_children(
        &mut self,
        gate_id: &str,
        new_parent_id: Option<Arc<str>>,
    ) -> Result<Vec<Arc<str>>> {
        // Get children before deletion
        let children: Vec<Arc<str>> = self.get_children(gate_id).into_iter().cloned().collect();

        if children.is_empty() {
            // No children, just delete the node
            self.remove_node(gate_id);
            return Ok(Vec::new());
        }

        // Reparent children
        if let Some(ref new_parent) = new_parent_id {
            // Check if new parent exists
            let all_gates: HashSet<Arc<str>> = self
                .children
                .keys()
                .chain(self.parents.keys())
                .cloned()
                .collect();

            if !all_gates.contains(new_parent) && new_parent.as_ref() != gate_id {
                return Err(anyhow!(
                    "new parent not found in hierarchy {}",
                    new_parent.as_ref(),
                ));
            }

            // Check for cycles
            for child in &children {
                if self.would_create_cycle(new_parent, child) {
                    return Err(anyhow!(
                        "failed to remove gate from hierarchy {} {}",
                        new_parent.as_ref(),
                        child.as_ref(),
                    ));
                }
            }

            // Reparent all children
            for child in &children {
                self.reparent(child.as_ref(), new_parent.clone())?;
            }
        } else {
            // Make children root nodes (remove their parent relationship)
            for child in &children {
                if self.parents.remove(child).is_some() {
                    // Also remove from old parent's children list
                    // (This is already handled by reparent, but we need to do it manually here)
                }
            }
        }

        // Now delete the node
        self.remove_node(gate_id);

        Ok(children)
    }

    /// Delete a single node, orphaning its children
    ///
    /// Removes a gate from the hierarchy. Its children become orphaned (root nodes).
    /// This is equivalent to `delete_node_keep_children(gate_id, None)`.
    ///
    /// # Arguments
    /// * `gate_id` - The ID of the gate to delete
    ///
    /// # Returns
    /// A vector of orphaned child IDs
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("parent", "child", 0);
    /// hierarchy.add_child("child", "grandchild", 0);
    ///
    /// let orphaned = hierarchy.delete_node("child")?;
    /// assert_eq!(orphaned.len(), 1); // "grandchild" is now orphaned
    /// assert!(hierarchy.is_root("grandchild"));
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn delete_node(&mut self, gate_id: &str) -> Result<Vec<Arc<str>>> {
        self.delete_node_keep_children(gate_id, None)
    }

    /// Add a child-parent relationship, returning a Result
    ///
    /// This is a convenience wrapper around `add_child` that returns an error
    /// instead of `false` when the operation fails.
    ///
    /// # Arguments
    /// * `parent_id` - The ID of the parent gate
    /// * `child_id` - The ID of the child gate
    ///
    /// # Returns
    /// `Ok(())` if successful, or an error if the operation would create a cycle
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_gate_child("parent", "child", None)?;
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn add_gate_child(
        &mut self,
        parent_id: impl Into<Arc<str>>,
        child_id: impl Into<Arc<str>>,
        order: Option<u64>,
    ) -> Result<()> {
        let parent_id = parent_id.into();
        let child_id = child_id.into();
        let ord = order.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u64
        });
        if !self.add_child(parent_id.clone(), child_id.clone(), ord) {
            return Err(anyhow!(
                "failed to add gate to hierarchy {} {}",
                parent_id.as_ref(),
                child_id.as_ref(),
            ));
        }

        Ok(())
    }

    /// Build a hierarchy from a list of gates and their relationships
    ///
    /// Creates a new hierarchy from a list of gates and their parent-child relationships.
    /// This is useful for constructing hierarchies programmatically.
    ///
    /// # Arguments
    /// * `relationships` - A slice of (parent_id, child_id) tuples defining the hierarchy
    ///
    /// # Returns
    /// A new `GateHierarchy` with the specified relationships, or an error if:
    /// - Any relationship would create a cycle
    /// - Relationships are invalid
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    /// use std::sync::Arc;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let relationships: Vec<(Arc<str>, Arc<str>, Option<u64>)> = vec![
    ///     (Arc::from("root"), Arc::from("child1"), Some(0)),
    ///     (Arc::from("root"), Arc::from("child2"), Some(1)),
    ///     (Arc::from("child1"), Arc::from("grandchild"), Some(0)),
    /// ];
    /// let hierarchy = GateHierarchy::from_relationships(&relationships)?;
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn from_relationships(relationships: &[(Arc<str>, Arc<str>, Option<u64>)]) -> Result<Self> {
        let mut hierarchy = Self::new();

        for (parent, child, ord) in relationships {
            hierarchy.add_gate_child(parent.clone(), child.clone(), *ord)?;
        }

        Ok(hierarchy)
    }

    /// Iterate gates in topological order (parents before children)
    ///
    /// Returns an iterator over gate IDs in topological order, where parents
    /// always come before their children. This is useful for processing gates
    /// in dependency order.
    ///
    /// # Returns
    /// An iterator over gate IDs in topological order, or an empty iterator if there are cycles
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("a", "b", 0);
    /// hierarchy.add_child("a", "c", 0);
    ///
    /// let order: Vec<_> = hierarchy.iter_topological().collect();
    /// // "a" will come before "b" and "c"
    /// ```
    pub fn iter_topological(&self) -> impl Iterator<Item = Arc<str>> {
        self.topological_sort().unwrap_or_default().into_iter()
    }

    /// Iterate gates in depth-first order starting from a root
    ///
    /// Returns an iterator over gate IDs in depth-first order, starting from
    /// the specified root gate and traversing down the tree.
    ///
    /// # Arguments
    /// * `root` - The root gate ID to start traversal from
    ///
    /// # Returns
    /// An iterator over gate IDs in depth-first order
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("root", "child1", 0);
    /// hierarchy.add_child("root", "child2", 0);
    /// hierarchy.add_child("child1", "grandchild", 0);
    ///
    /// let order: Vec<_> = hierarchy.iter_dfs("root").collect();
    /// // Order: root, child1, grandchild, child2 (or similar DFS order)
    /// ```
    pub fn iter_dfs(&self, root: &str) -> impl Iterator<Item = Arc<str>> {
        let mut stack: Vec<Arc<str>> = vec![Arc::from(root)];
        let mut visited = HashSet::new();
        std::iter::from_fn(move || {
            while let Some(node) = stack.pop() {
                if visited.insert(node.clone()) {
                    // Add children to stack in reverse order to maintain left-to-right traversal
                    if let Some(children) = self.children.get(&node) {
                        for child in children.iter().rev() {
                            stack.push(child.clone());
                        }
                    }
                    return Some(node);
                }
            }
            None
        })
    }

    /// Validate the hierarchy structure
    ///
    /// Checks the hierarchy for common issues:
    /// - Cycles
    /// - Orphaned gates (gates with no parent and not in children map)
    /// - Inconsistent parent-child relationships
    ///
    /// # Returns
    /// `Ok(())` if the hierarchy is valid, or an error describing the issue
    ///
    /// # Example
    /// ```rust
    /// use clingate::gate_editor::gates::gate_hierarchy::GateHierarchy;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut hierarchy = GateHierarchy::new();
    /// hierarchy.add_child("parent", "child", 0);
    /// hierarchy.validate()?; // Should pass
    /// # Ok(())
    /// # }
    /// # example().unwrap();
    /// ```
    pub fn validate(&self) -> Result<()> {
        // Check for cycles using topological sort
        let all_gates: HashSet<Arc<str>> = self
            .children
            .keys()
            .chain(self.parents.keys())
            .cloned()
            .collect();

        if let Some(sorted) = self.topological_sort() {
            if sorted.len() != all_gates.len() {
                return Err(anyhow!(
                    "Topological sort failed - possible cycles detected",
                ));
            }
        } else {
            return Err(anyhow!("Cycles detected in hierarchy"));
        }

        // Check for inconsistent relationships
        for (parent, children) in &self.children {
            for child in children {
                if let Some(child_parent) = self.parents.get(child) {
                    if child_parent != parent {
                        return Err(anyhow!(
                            "Inconsistent relationship: {} is child of {} but parent is {}",
                            child.as_ref(),
                            parent.as_ref(),
                            child_parent.as_ref()
                        ));
                    }
                } else {
                    return Err(anyhow!("Child {} has no parent entry", child.as_ref()));
                }
            }
        }

        Ok(())
    }
}

//cargo test gate_hierarchy_tests -- --nocapture
// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod gate_hierarchy_tests {
    use super::*;

    /// root -> a -> b -> c, plus a second branch root -> x
    fn linear_tree() -> GateHierarchy {
        let mut h = GateHierarchy::new();
        h.add_child("root", "a", 0);
        h.add_child("a", "b", 0);
        h.add_child("b", "c", 0);
        h.add_child("root", "x", 1);
        h
    }

    fn ids(v: Vec<Arc<str>>) -> Vec<String> {
        v.into_iter().map(|a| a.to_string()).collect()
    }

    fn sorted_ids(v: Vec<Arc<str>>) -> Vec<String> {
        let mut s = ids(v);
        s.sort();
        s
    }

    // ── Construction and basic relationships ──────────────────────────────────

    #[test]
    fn new_hierarchy_is_empty() {
        let h = GateHierarchy::new();
        assert!(h.get_roots().is_empty());
        assert!(h.get_leaves().is_empty());
        assert_eq!(h.topological_sort().map(|v| v.len()), Some(0));
    }

    #[test]
    fn add_child_links_both_directions() {
        let mut h = GateHierarchy::new();
        assert!(h.add_child("parent", "child", 0));

        assert_eq!(
            h.get_parent("child").map(|p| p.to_string()),
            Some("parent".to_string())
        );
        assert_eq!(
            ids(h.get_children("parent").into_iter().cloned().collect()),
            vec!["child"]
        );
    }

    #[test]
    fn add_root_registers_a_parentless_gate() {
        let mut h = GateHierarchy::new();
        h.add_root("solo");

        assert!(h.is_root("solo"));
        assert!(h.is_leaf("solo"));
        assert_eq!(ids(h.get_roots()), vec!["solo"]);
    }

    #[test]
    fn ancestors_run_from_closest_to_root() {
        let h = linear_tree();
        assert_eq!(ids(h.get_ancestors("c")), vec!["b", "a", "root"]);
    }

    #[test]
    fn chain_to_root_runs_root_first_and_includes_self() {
        let h = linear_tree();
        assert_eq!(ids(h.get_chain_to_root("c")), vec!["root", "a", "b", "c"]);
    }

    /// The gating chain is what filter_events_by_hierarchy_to_mask ANDs together,
    /// so a root gate must still yield itself rather than an empty chain.
    #[test]
    fn chain_to_root_of_a_root_is_just_itself() {
        let h = linear_tree();
        assert_eq!(ids(h.get_chain_to_root("root")), vec!["root"]);
    }

    #[test]
    fn descendants_include_the_whole_subtree_but_not_self() {
        let h = linear_tree();
        assert_eq!(sorted_ids(h.get_descendants("a")), vec!["b", "c"]);
        assert!(h.get_descendants("c").is_empty());
    }

    #[test]
    fn depth_counts_ancestors_with_root_at_zero() {
        let h = linear_tree();
        assert_eq!(h.get_depth("root"), 0);
        assert_eq!(h.get_depth("a"), 1);
        assert_eq!(h.get_depth("c"), 3);
    }

    #[test]
    fn roots_and_leaves_are_identified() {
        let h = linear_tree();
        assert_eq!(ids(h.get_roots()), vec!["root"]);
        assert_eq!(sorted_ids(h.get_leaves()), vec!["c", "x"]);
        assert!(h.is_root("root"));
        assert!(!h.is_root("a"));
        assert!(h.is_leaf("c"));
        assert!(!h.is_leaf("b"));
    }

    /// A gate that was never added is vacuously a leaf and a root - callers rely
    /// on this not panicking when probing an unknown id.
    #[test]
    fn unknown_gate_queries_do_not_panic() {
        let h = linear_tree();
        assert!(h.get_parent("nope").is_none());
        assert!(h.get_children("nope").is_empty());
        assert!(h.get_descendants("nope").is_empty());
        assert!(h.get_ancestors("nope").is_empty());
        assert!(h.is_leaf("nope"));
        assert!(h.is_root("nope"));
    }

    // ── Sibling ordering (Omiq's `ord` field) ─────────────────────────────────

    #[test]
    fn children_are_kept_sorted_by_order() {
        let mut h = GateHierarchy::new();
        h.add_child("p", "third", 30);
        h.add_child("p", "first", 10);
        h.add_child("p", "second", 20);

        assert_eq!(
            ids(h.get_children("p").into_iter().cloned().collect()),
            vec!["first", "second", "third"]
        );
    }

    #[test]
    fn reordering_applies_when_a_child_is_re_added() {
        let mut h = GateHierarchy::new();
        h.add_child("p", "a", 10);
        h.add_child("p", "b", 20);
        // Re-adding with a lower order should move it to the front.
        h.add_child("p", "b", 5);

        assert_eq!(
            ids(h.get_children("p").into_iter().cloned().collect()),
            vec!["b", "a"]
        );
    }

    #[test]
    fn add_gate_child_defaults_the_order_when_none_given() {
        let mut h = GateHierarchy::new();
        h.add_gate_child("p", "a", None).unwrap();
        assert_eq!(
            h.get_parent("a").map(|p| p.to_string()),
            Some("p".to_string())
        );
    }

    // ── Cycle prevention ──────────────────────────────────────────────────────

    #[test]
    fn direct_cycle_is_rejected() {
        let mut h = GateHierarchy::new();
        h.add_child("a", "b", 0);
        assert!(!h.add_child("b", "a", 0));
        assert_eq!(
            h.get_parent("b").map(|p| p.to_string()),
            Some("a".to_string())
        );
    }

    #[test]
    fn indirect_cycle_is_rejected() {
        let h = linear_tree();
        let mut h = h;
        // root -> a -> b -> c; making root a child of c closes the loop.
        assert!(!h.add_child("c", "root", 0));
    }

    #[test]
    fn add_gate_child_reports_a_cycle_as_an_error() {
        let mut h = GateHierarchy::new();
        h.add_child("a", "b", 0);
        assert!(h.add_gate_child("b", "a", None).is_err());
    }

    // ── Moving a child between parents ────────────────────────────────────────

    #[test]
    fn adding_an_existing_child_moves_it_off_its_old_parent() {
        let mut h = GateHierarchy::new();
        h.add_child("p1", "child", 0);
        h.add_child("p2", "child", 0);

        assert_eq!(
            h.get_parent("child").map(|p| p.to_string()),
            Some("p2".to_string())
        );
        assert!(
            h.get_children("p1").is_empty(),
            "stale child left on the old parent"
        );
        assert_eq!(
            ids(h.get_children("p2").into_iter().cloned().collect()),
            vec!["child"]
        );
    }

    #[test]
    fn reparent_moves_a_gate_and_keeps_the_tree_valid() {
        let mut h = linear_tree();
        h.reparent("c", "x").unwrap();

        assert_eq!(
            h.get_parent("c").map(|p| p.to_string()),
            Some("x".to_string())
        );
        assert!(h.get_children("b").is_empty());
        h.validate().unwrap();
    }

    #[test]
    fn reparent_rejects_an_unknown_gate() {
        let mut h = linear_tree();
        assert!(h.reparent("ghost", "root").is_err());
    }

    #[test]
    fn reparent_subtree_carries_the_descendants_along() {
        let mut h = linear_tree();
        h.reparent_subtree("a", "x").unwrap();

        assert_eq!(
            h.get_parent("a").map(|p| p.to_string()),
            Some("x".to_string())
        );
        // b and c must still hang off a.
        assert_eq!(
            ids(h.get_chain_to_root("c")),
            vec!["root", "x", "a", "b", "c"]
        );
        h.validate().unwrap();
    }

    #[test]
    fn reparent_subtree_rejects_a_move_into_its_own_descendant() {
        let mut h = linear_tree();
        assert!(h.reparent_subtree("a", "c").is_err());
        // The tree must be untouched after the rejection.
        assert_eq!(ids(h.get_chain_to_root("c")), vec!["root", "a", "b", "c"]);
    }

    // ── Deletion ──────────────────────────────────────────────────────────────

    #[test]
    fn delete_subtree_returns_the_gate_and_all_descendants() {
        let mut h = linear_tree();
        let deleted = sorted_ids(h.delete_subtree("a"));

        assert_eq!(deleted, vec!["a", "b", "c"]);
        assert!(h.get_parent("a").is_none());
        assert!(h.get_parent("c").is_none());
        assert!(h.get_children("root").len() == 1, "x should survive");
    }

    /// gate_store::remove_gate reads each doomed gate's parent to clean up the
    /// per-plot view index. delete_subtree unlinks as it goes, so the parent is
    /// only available beforehand - this pins that behaviour down.
    #[test]
    fn delete_subtree_unlinks_parents_so_they_must_be_read_first() {
        let mut h = linear_tree();
        assert_eq!(
            h.get_parent("b").map(|p| p.to_string()),
            Some("a".to_string())
        );

        h.delete_subtree("a");

        assert!(
            h.get_parent("b").is_none(),
            "parent lookups after deletion silently fall back to the root"
        );
    }

    #[test]
    fn delete_node_keep_children_reparents_to_the_grandparent() {
        let mut h = linear_tree();
        let moved = h
            .delete_node_keep_children("b", Some(Arc::from("a")))
            .unwrap();

        assert_eq!(ids(moved), vec!["c"]);
        assert_eq!(
            h.get_parent("c").map(|p| p.to_string()),
            Some("a".to_string())
        );
        assert!(h.get_parent("b").is_none());
        h.validate().unwrap();
    }

    #[test]
    fn delete_node_orphans_the_children_when_no_new_parent_given() {
        let mut h = linear_tree();
        h.delete_node("b").unwrap();

        assert!(h.get_parent("c").is_none());
        assert!(h.is_root("c"));
    }

    #[test]
    fn delete_node_on_a_leaf_removes_just_that_leaf() {
        let mut h = linear_tree();
        let moved = h.delete_node("c").unwrap();

        assert!(moved.is_empty());
        assert!(h.get_parent("c").is_none());
        assert!(h.is_leaf("b"));
    }

    #[test]
    fn remove_child_detaches_only_that_link() {
        let mut h = linear_tree();
        h.remove_child("a", "b");

        assert!(h.get_children("a").is_empty());
        assert!(h.get_parent("b").is_none());
        // c is still attached to b.
        assert_eq!(
            ids(h.get_children("b").into_iter().cloned().collect()),
            vec!["c"]
        );
    }

    #[test]
    fn clear_empties_the_hierarchy() {
        let mut h = linear_tree();
        h.clear();

        assert!(h.get_roots().is_empty());
        assert!(h.get_parent("c").is_none());
    }

    // ── Ordering guarantees relied on by the importer ─────────────────────────

    #[test]
    fn topological_sort_puts_parents_before_children() {
        let h = linear_tree();
        let order = ids(h.topological_sort().expect("acyclic"));

        let pos = |id: &str| order.iter().position(|o| o == id).expect("present");
        assert!(pos("root") < pos("a"));
        assert!(pos("a") < pos("b"));
        assert!(pos("b") < pos("c"));
        assert!(pos("root") < pos("x"));
    }

    #[test]
    fn iter_topological_agrees_with_topological_sort() {
        let h = linear_tree();
        assert_eq!(
            ids(h.iter_topological().collect()),
            ids(h.topological_sort().unwrap())
        );
    }

    #[test]
    fn iter_dfs_walks_the_subtree_from_the_given_root() {
        let h = linear_tree();
        let walked = sorted_ids(h.iter_dfs("a").collect());
        assert!(walked.contains(&"b".to_string()));
        assert!(walked.contains(&"c".to_string()));
        assert!(!walked.contains(&"x".to_string()));
    }

    // ── Bulk construction and validation ──────────────────────────────────────

    #[test]
    fn from_relationships_builds_the_tree() {
        let rels: Vec<(Arc<str>, Arc<str>, Option<u64>)> = vec![
            (Arc::from("root"), Arc::from("a"), Some(0)),
            (Arc::from("a"), Arc::from("b"), Some(0)),
        ];
        let h = GateHierarchy::from_relationships(&rels).unwrap();

        assert_eq!(ids(h.get_chain_to_root("b")), vec!["root", "a", "b"]);
        h.validate().unwrap();
    }

    #[test]
    fn from_relationships_rejects_a_cycle() {
        let rels: Vec<(Arc<str>, Arc<str>, Option<u64>)> = vec![
            (Arc::from("a"), Arc::from("b"), Some(0)),
            (Arc::from("b"), Arc::from("a"), Some(0)),
        ];
        assert!(GateHierarchy::from_relationships(&rels).is_err());
    }

    /// BUG (docs/test-audit.md, B-HIER-1): both branches of the check after
    /// `add_child` return an error - the failure branch says "possible
    /// cycle", the success branch "no order for child" - so cloning any
    /// subtree with a child in it fails. Nothing calls it yet; its doctest
    /// never noticed because the example was never run.
    #[test]
    #[ignore = "known bug B-HIER-1: clone_subtree fails on every subtree with a child"]
    fn a_subtree_can_be_cloned_under_new_ids() {
        let mut h = GateHierarchy::new();
        h.add_child("parent", "child", 0);
        h.add_child("child", "grandchild", 0);
        let cloned = h.clone_subtree("child", |id| format!("{id}_copy")).unwrap();
        assert_eq!(
            cloned.get_parent("grandchild_copy").map(|p| p.as_ref()),
            Some("child_copy")
        );
        assert!(
            cloned.get_parent("child").is_none(),
            "the originals are not copied"
        );
    }

    /// BUG (docs/test-audit.md, B-HIER-2): `would_create_cycle` asks whether
    /// the new parent is among the child's descendants, and a gate is not its
    /// own descendant - so a gate can be made its own parent, by `add_child`
    /// or by `reparent`. Walking up from it (`get_ancestors`, every gate
    /// chain) then never ends.
    #[test]
    #[ignore = "known bug B-HIER-2: a gate can be made its own parent"]
    fn a_gate_cannot_be_its_own_parent() {
        let mut h = GateHierarchy::new();
        assert!(!h.add_child("g", "g", 0), "add_child accepted a self-edge");
        h.add_child("root", "a", 0);
        assert!(
            h.reparent("a", "a").is_err(),
            "reparent accepted a self-edge"
        );
        assert!(h.validate().is_ok());

        // The same gap, reached by deleting a gate and handing its children
        // to one of those children - found by the random sequences below.
        let mut h = GateHierarchy::new();
        h.add_child("g6", "g7", 0);
        let _ = h.delete_node_keep_children("g6", Some(Arc::from("g7")));
        assert!(h.validate().is_ok(), "g7 was made its own parent");
    }

    /// Random sequences of every editing operation, checked after each step:
    /// the tree must stay valid - no cycles, every child's parent entry
    /// agreeing with its parent's child list - whatever order the edits
    /// come in. Seeded, so a failure names the sequence that caused it.
    #[test]
    fn any_sequence_of_edits_leaves_a_valid_tree() {
        use rand::prelude::*;
        let names: Vec<String> = (0..12).map(|i| format!("g{i}")).collect();
        for seed in 0..2_000u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let mut h = GateHierarchy::new();
            let mut log = Vec::new();
            for _ in 0..40 {
                let a = names[rng.random_range(0..names.len())].clone();
                let b = names[rng.random_range(0..names.len())].clone();
                if a == b {
                    // A self-edge is B-HIER-2, pinned on its own above;
                    // skipped here so the sequences can find anything else.
                    continue;
                }
                let step = match rng.random_range(0..6) {
                    0 => {
                        h.add_child(a.as_str(), b.as_str(), rng.random_range(0..5));
                        format!("add_child({a}, {b})")
                    }
                    1 => {
                        let _ = h.reparent(b.as_str(), a.as_str());
                        format!("reparent({b}, {a})")
                    }
                    2 => {
                        let _ = h.reparent_subtree(b.as_str(), a.as_str());
                        format!("reparent_subtree({b}, {a})")
                    }
                    3 => {
                        h.delete_subtree(&a);
                        format!("delete_subtree({a})")
                    }
                    4 => {
                        if h.get_children(&a).iter().any(|c| c.as_ref() == b) {
                            // Handing a gate's children to one of them is
                            // B-HIER-2 again; pinned above.
                            continue;
                        }
                        let _ = h.delete_node_keep_children(&a, Some(Arc::from(b.as_str())));
                        format!("delete_node_keep_children({a}, Some({b}))")
                    }
                    _ => {
                        let _ = h.delete_node(&a);
                        format!("delete_node({a})")
                    }
                };
                log.push(step);
                if let Err(e) = h.validate() {
                    panic!("seed {seed}: {e}\nafter: {}", log.join(", "));
                }
            }
        }
    }

    #[test]
    fn validate_accepts_a_well_formed_tree() {
        linear_tree().validate().unwrap();
    }

    // ── A realistic gating tree ───────────────────────────────────────────────

    /// Mirrors a typical imported panel: singlets -> live -> CD3 -> {CD4, CD8},
    /// with a quadrant's four subgates hanging off CD4.
    #[test]
    fn realistic_panel_resolves_chains_and_ordering() {
        let mut h = GateHierarchy::new();
        h.add_gate_child("root", "singlets", Some(0)).unwrap();
        h.add_gate_child("singlets", "live", Some(0)).unwrap();
        h.add_gate_child("live", "cd3", Some(0)).unwrap();
        h.add_gate_child("cd3", "cd4", Some(0)).unwrap();
        h.add_gate_child("cd3", "cd8", Some(1)).unwrap();
        for (i, q) in ["q_bl", "q_br", "q_tr", "q_tl"].iter().enumerate() {
            h.add_gate_child("cd4", *q, Some(i as u64)).unwrap();
        }

        assert_eq!(
            ids(h.get_chain_to_root("q_tr")),
            vec!["root", "singlets", "live", "cd3", "cd4", "q_tr"]
        );
        assert_eq!(
            ids(h.get_children("cd4").into_iter().cloned().collect()),
            vec!["q_bl", "q_br", "q_tr", "q_tl"]
        );
        assert_eq!(
            sorted_ids(h.get_leaves()),
            vec!["cd8", "q_bl", "q_br", "q_tl", "q_tr"]
        );
        h.validate().unwrap();

        // Deleting the quadrant's parent takes all four subgates with it.
        let deleted = sorted_ids(h.delete_subtree("cd4"));
        assert_eq!(deleted, vec!["cd4", "q_bl", "q_br", "q_tl", "q_tr"]);
        h.validate().unwrap();
    }
}
