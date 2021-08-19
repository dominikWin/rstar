use std::mem::replace;

use crate::algorithm::selection_functions::SelectionFunction;
use crate::node::{ParentNode, RTreeNode};
use crate::object::RTreeObject;
use crate::params::RTreeParams;
use crate::RTree;

/// Default removal strategy to remove elements from an r-tree. A [RemovalFunction]
/// specifies which elements shall be removed.
///
/// The algorithm descends the tree to the leaf level, using the given removal function
/// (see [SelectionFunc]). Then, the removal function defines which leaf node shall be
/// removed. Once the first node is found, the process stops and the element is removed and
/// returned.
///
/// If a tree node becomes empty due to this removal, it is also removed from its parent node.
pub fn remove<T, Params, R>(node: &mut ParentNode<T>, removal_function: &R) -> Option<T>
where
    T: RTreeObject,
    Params: RTreeParams,
    R: SelectionFunction<T>,
{
    remove_recursive::<_, Params, _>(node, removal_function, true).pop()
}

pub fn remove_all<T, Params, R>(node: &mut ParentNode<T>, removal_function: &R) -> Vec<T>
where
    T: RTreeObject,
    Params: RTreeParams,
    R: SelectionFunction<T>,
{
    remove_recursive::<_, Params, _>(node, removal_function, false)
}

fn remove_recursive<T, Params, R>(
    node: &mut ParentNode<T>,
    removal_function: &R,
    remove_only_first: bool,
) -> Vec<T>
where
    T: RTreeObject,
    Params: RTreeParams,
    R: SelectionFunction<T>,
{
    let mut result = Vec::new();
    if removal_function.should_unpack_parent(&node.envelope) {
        let mut i = 0;
        while i < node.children.len() {
            let child = &mut node.children[i];
            match child {
                RTreeNode::Parent(ref mut data) => {
                    result.append(&mut remove_recursive::<_, Params, _>(
                        data,
                        removal_function,
                        remove_only_first,
                    ));
                    if !result.is_empty() {
                        // Mark child for removal if it has become empty
                        if data.children.is_empty() {
                            node.children.remove(i);
                        }
                        if remove_only_first {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                RTreeNode::Leaf(ref b) => {
                    if removal_function.should_unpack_leaf(b) {
                        // Mark leaf for removal if should be removed
                        let val = node.children.remove(i);
                        if let RTreeNode::Leaf(t) = val {
                            result.push(t);
                        } else {
                            unreachable!("This is a bug.");
                        }
                        if remove_only_first {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }
    if !result.is_empty() {
        // Update the envelope, it may have become smaller
        node.envelope = crate::node::envelope_for_children(&node.children);
    }
    result
}

pub(crate) struct DrainIterator<'a, T, R, Params>
where
    T: RTreeObject,
    Params: RTreeParams,
    R: SelectionFunction<T>,
{
    node_stack: Vec<(ParentNode<T>, usize, usize)>,
    removal_function: R,
    rtree: &'a mut RTree<T, Params>,
    original_size: usize,
}

impl<'a, T, R, Params> DrainIterator<'a, T, R, Params>
where
    T: RTreeObject,
    Params: RTreeParams,
    R: SelectionFunction<T>,
{
    pub(crate) fn new(rtree: &'a mut RTree<T, Params>, removal_function: R) -> Self {
        // We replace with a brand new RTree in case the iterator is
        // `mem::forgot`ten.
        let RTree { root, size, .. } = replace(rtree, RTree::new_with_params());

        DrainIterator {
            node_stack: vec![(root, 0, 0)],
            original_size: size,
            removal_function, rtree,
        }
    }

    fn pop_node(&mut self, increment_idx: bool) -> Option<(ParentNode<T>, usize)> {
        debug_assert!(!self.node_stack.is_empty());

        let (mut node, _, num_removed) = self.node_stack.pop().unwrap();

        // We only compute envelope for the current node as the parent
        // is taken care of when it is popped.

        // TODO: May be make this a method on `ParentNode`
        if num_removed > 0 {
            node.envelope = crate::node::envelope_for_children(&node.children);
        }

        // If there is no parent, this is the new root node to set back in the rtree
        // O/w, get the new top in stack
        let (parent_node, parent_idx, parent_removed) = match self.node_stack.last_mut() {
            Some(pn) => (&mut pn.0, &mut pn.1, &mut pn.2),
            None => return Some((node, num_removed)),
        };

        // Update the remove count on parent
        *parent_removed += num_removed;

        // If the node has no children, we don't need to add it back to the parent
        if node.children.is_empty() { return None; }

        // Put the child back (but re-arranged)
        parent_node.children.push(RTreeNode::Parent(node));

        // Swap it with the current item and increment idx.

        // A minor optimization is to avoid the swap in the destructor,
        // where we aren't going to be iterating any more.
        if !increment_idx { return None; }

        // Note that during iteration, parent_idx may be equal to
        // (previous) children.len(), but this is okay as the swap will be
        // a no-op.
        let parent_len = parent_node.children.len();
        parent_node.children.swap(*parent_idx, parent_len - 1);
        *parent_idx += 1;

        None
    }
}

impl<'a, T, R, Params> Iterator for DrainIterator<'a, T, R, Params>
where
    T: RTreeObject,
    Params: RTreeParams,
    R: SelectionFunction<T>,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        let (node, idx, remove_count) = match self.node_stack.last_mut() {
            Some(node) => (&mut node.0, &mut node.1, &mut node.2),
            None => return None,
        };

        if *idx > 0 || self.removal_function.should_unpack_parent(&node.envelope) {
            while *idx < node.children.len() {
                match &mut node.children[*idx] {
                    RTreeNode::Parent(_) => {
                        // Swap node with last, remove and return the value.
                        // No need to increment idx as something else has replaced it;
                        // or idx == new len, and we'll handle it in the next iteration.
                        let child = match node.children.swap_remove(*idx) {
                            RTreeNode::Leaf(_) => unreachable!("DrainIterator bug!"),
                            RTreeNode::Parent(node) => node,
                        };
                        self.node_stack.push((child, 0, 0));
                        return self.next();
                    }
                    RTreeNode::Leaf(ref leaf) => {
                        if self.removal_function.should_unpack_leaf(leaf) {
                            // Swap node with last, remove and return the value.
                            // No need to increment idx as something else has replaced it;
                            // or idx == new len, and we'll handle it in the next iteration.
                            *remove_count += 1;
                            return match node.children.swap_remove(*idx) {
                                RTreeNode::Leaf(data) => Some(data),
                                _ => unreachable!("RemovalIterator bug!"),
                            };
                        }
                        *idx += 1;
                    }
                }
            }
        }

        if let Some((new_root, total_removed)) = self.pop_node(true) {
            // This happens if we are done with the iteration.
            // Set the root back in rtree and return None
            self.rtree.root = new_root;
            self.rtree.size = self.original_size - total_removed;
            return None;
        }

        // TODO: fix tail recursion (it's only log-n depth though)
        self.next()
    }
}

impl<'a, T, R, Params> Drop for DrainIterator<'a, T, R, Params>
where
    T: RTreeObject,
    Params: RTreeParams,
    R: SelectionFunction<T>,
{
    fn drop(&mut self) {
        // Re-assemble back the original rtree and update envelope as we
        // re-assemble.
        if self.node_stack.is_empty() {
            // The iteration handled everything, nothing to do.
            return;
        }

        loop {
            debug_assert!(!self.node_stack.is_empty());
            if let Some((new_root, total_removed)) = self.pop_node(false) {
                self.rtree.root = new_root;
                self.rtree.size = self.original_size - total_removed;
                break;
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::algorithm::selection_functions::{SelectAllFunc, SelectInEnvelopeFuncIntersecting};
    use crate::point::PointExt;
    use crate::primitives::Line;
    use crate::test_utilities::{create_random_points, create_random_rectangles, SEED_1, SEED_2};
    use crate::{AABB, RTree};

    use super::*;

    #[test]
    fn test_remove_and_insert() {
        const SIZE: usize = 1000;
        let points = create_random_points(SIZE, SEED_1);
        let later_insertions = create_random_points(SIZE, SEED_2);
        let mut tree = RTree::bulk_load(points.clone());
        for (point_to_remove, point_to_add) in points.iter().zip(later_insertions.iter()) {
            assert!(tree.remove_at_point(point_to_remove).is_some());
            tree.insert(*point_to_add);
        }
        assert_eq!(tree.size(), SIZE);
        assert!(points.iter().all(|p| !tree.contains(p)));
        assert!(later_insertions.iter().all(|p| tree.contains(p)));
        for point in &later_insertions {
            assert!(tree.remove_at_point(point).is_some());
        }
        assert_eq!(tree.size(), 0);
    }

    #[test]
    fn test_remove_and_insert_rectangles() {
        const SIZE: usize = 1000;
        let initial_rectangles = create_random_rectangles(SIZE, SEED_1);
        let new_rectangles = create_random_rectangles(SIZE, SEED_2);
        let mut tree = RTree::bulk_load(initial_rectangles.clone());

        for (rectangle_to_remove, rectangle_to_add) in
            initial_rectangles.iter().zip(new_rectangles.iter())
        {
            assert!(tree.remove(rectangle_to_remove).is_some());
            tree.insert(*rectangle_to_add);
        }
        assert_eq!(tree.size(), SIZE);
        assert!(initial_rectangles.iter().all(|p| !tree.contains(p)));
        assert!(new_rectangles.iter().all(|p| tree.contains(p)));
        for rectangle in &new_rectangles {
            assert!(tree.contains(rectangle));
        }
        for rectangle in &initial_rectangles {
            assert!(!tree.contains(rectangle));
        }
        for rectangle in &new_rectangles {
            assert!(tree.remove(rectangle).is_some());
        }
        assert_eq!(tree.size(), 0);
    }

    #[test]
    fn test_remove_at_point() {
        let points = create_random_points(1000, SEED_1);
        let mut tree = RTree::bulk_load(points.clone());
        for point in &points {
            let size_before_removal = tree.size();
            assert!(tree.remove_at_point(point).is_some());
            assert!(tree.remove_at_point(&[1000.0, 1000.0]).is_none());
            assert_eq!(size_before_removal - 1, tree.size());
        }
    }

    #[test]
    fn test_remove() {
        let points = create_random_points(1000, SEED_1);
        let offsets = create_random_points(1000, SEED_2);
        let scaled = offsets.iter().map(|p| p.mul(0.05));
        let edges: Vec<_> = points
            .iter()
            .zip(scaled)
            .map(|(from, offset)| Line::new(*from, from.add(&offset)))
            .collect();
        let mut tree = RTree::bulk_load(edges.clone());
        for edge in &edges {
            let size_before_removal = tree.size();
            assert!(tree.remove(edge).is_some());
            assert!(tree.remove(edge).is_none());
            assert_eq!(size_before_removal - 1, tree.size());
        }
    }

    #[test]
    fn test_drain_iterator() {
        const SIZE: usize = 1000;
        let points = create_random_points(SIZE, SEED_1);
        let mut tree = RTree::bulk_load(points.clone());

        let drain_count = DrainIterator::new(&mut tree, SelectAllFunc).take(250).count();
        assert_eq!(drain_count, 250);
        assert_eq!(tree.size(), 750);

        let drain_count = DrainIterator::new(&mut tree, SelectAllFunc).count();
        assert_eq!(drain_count, 750);
        assert_eq!(tree.size(), 0);

        let points = create_random_points(1000, SEED_1);
        let mut tree = RTree::bulk_load(points.clone());

        // The total for this is 406 (for SEED_1)
        let env = AABB::from_corners([-2., -0.6], [0.5, 0.85]);

        let sel = SelectInEnvelopeFuncIntersecting::new(env);
        let drain_count = DrainIterator::new(&mut tree, sel).take(80).count();
        assert_eq!(drain_count, 80);

        let sel = SelectInEnvelopeFuncIntersecting::new(env);
        let drain_count = DrainIterator::new(&mut tree, sel).count();
        assert_eq!(drain_count, 326);

        let sel = SelectInEnvelopeFuncIntersecting::new(env);
        let sel_count = tree.locate_with_selection_function(sel).count();
        assert_eq!(sel_count, 0);
        assert_eq!(tree.size(), 1000 - 80 - 326);
    }
}
