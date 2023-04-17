use crate::op_tree::{LastInsert, OpTreeNode};
use crate::query::QueryResult;
use crate::types::{Key, ListEncoding, Op};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListState {
    pub(crate) encoding: ListEncoding,
    pub(crate) last_seen: Option<Key>,
    pub(crate) last_width: usize,
    pub(crate) target: usize,
    pub(crate) index: usize,
    pub(crate) pos: usize,
    pub(crate) clean: bool,
}

// There are two indexes being tracked in lists
// pos: this is the position in the opset.  A list of 100 items may have 1000 ops.  Each op has a position
// index: this is the logical index of the list of visible values.  Conflicted items will have the same index.
//        The index is affected by utf8/utf16 encoding for text, etc

impl ListState {
    pub(crate) fn new(encoding: ListEncoding, target: usize) -> Self {
        ListState {
            encoding,
            target,
            last_seen: None,
            last_width: 0,
            index: 0,
            pos: 0,
            clean: true,
        }
    }

    pub(crate) fn check_if_node_is_clean(&mut self, node: &OpTreeNode) {
        self.clean &= node.index.clean;
    }

    pub(crate) fn process_node(&mut self, node: &OpTreeNode, ops: &[Op]) -> QueryResult {
        if self.encoding == ListEncoding::List {
            self.process_list_node(node, ops)
        } else if self.clean {
            // text node is clean - use the indexes
            self.process_text_node(node)
        } else {
            // text nodes are intended to only be interacted with splice()
            // meaning all ops are inserts or deleted inserts
            // the indexes are written with this assumption in mind
            // if conflicted put()'s with different character widths exist
            // we cannot trust the indexs and need to descend
            QueryResult::Descend
        }
    }

    fn process_text_node(&mut self, node: &OpTreeNode) -> QueryResult {
        let num_vis = node.index.visible_len(self.encoding);
        if self.index + num_vis >= self.target {
            return QueryResult::Descend;
        }
        self.index += num_vis;
        self.pos += node.len();
        QueryResult::Next
    }

    fn process_list_node(&mut self, node: &OpTreeNode, ops: &[Op]) -> QueryResult {
        let mut num_vis = node.index.visible.len();
        if let Some(last_seen) = self.last_seen {
            // the elemid `last_seen` is counted in this node's index
            // but since we've already seen it we dont want to count it again
            // as this is only for lists we can subtract 1
            if node.index.has_visible(&last_seen) {
                num_vis -= 1;
            }
        }

        if self.index + num_vis >= self.target {
            // if we've reached out target - decend
            return QueryResult::Descend;
        }

        self.index += num_vis;
        self.pos += node.len();

        // scenario 1: the last elemid is in the index - record it
        // scenario 2: the last elemid this node is not in the index
        // scenario 2a: and its different than the previous elemid - last_seen = None
        //              as there is no possability of visible elements spanning the
        //              node boundary
        // scenario 2b: it is the same as the previous last_seen - do nothing

        let last_elemid = ops[node.last()].elemid_or_key();
        if node.index.has_visible(&last_elemid) {
            self.last_seen = Some(last_elemid);
        } else if self.last_seen.is_some() && Some(last_elemid) != self.last_seen {
            self.last_seen = None;
        }
        QueryResult::Next
    }

    pub(crate) fn process_op(&mut self, op: &Op, current: Key, visible: bool) {
        if visible {
            if self.clean {
                // clean sequnces are simple - only insert and deletes
                self.last_width = op.width(self.encoding);
                self.index += self.last_width;
            } else {
                //let current = Some(op.elemid_or_key());
                let current_width = op.width(self.encoding);
                if self.last_seen != Some(current) {
                    // new value - progess
                    self.last_width = current_width;
                    self.index += self.last_width;
                    self.last_seen = Some(current);
                } else if current_width != self.last_width {
                    // width is always 1 for lists so this
                    // will only trigger if there are conflicting unicode characters
                    // of different lengths
                    self.index = self.index + current_width - self.last_width;
                    self.last_width = current_width;
                }
            }
        }
        self.pos += 1;
    }

    pub(crate) fn last_index(&self) -> usize {
        self.index - self.last_width
    }

    pub(crate) fn done(&self) -> bool {
        self.index >= self.target
    }

    pub(crate) fn seek(&mut self, last: &LastInsert) {
        self.last_width = last.width;
        self.index = last.index + last.width;
        self.pos = last.pos + 1;
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }
}