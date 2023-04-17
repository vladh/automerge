use crate::op_tree::OpTreeIter;
use crate::types::{Clock, Key, Op};

#[derive(Default)]
pub(crate) struct TopOps<'a> {
    iter: OpTreeIter<'a>,
    pos: usize,
    start_pos: usize,
    num_ops: usize,
    clock: Option<Clock>,
    key: Option<Key>,
    last_op: Option<(usize, &'a Op)>,
}

impl<'a> TopOps<'a> {
    pub(crate) fn new(iter: OpTreeIter<'a>, clock: Option<Clock>) -> Self {
        Self {
            iter,
            pos: 0,
            start_pos: 0,
            num_ops: 0,
            clock,
            key: None,
            last_op: None,
        }
    }
}

impl<'a> Iterator for TopOps<'a> {
    type Item = &'a Op;

    fn next(&mut self) -> Option<Self::Item> {
        let mut result = None;
        loop {
            if let Some(op) = self.iter.next() {
                let key = op.elemid_or_key();
                let visible = op.visible_at(self.clock.as_ref());
                match &self.key {
                    Some(k) if k == &key => {
                        if visible {
                            self.last_op = Some((self.pos, op));
                        }
                        self.num_ops += 1;
                    }
                    Some(_) => {
                        result = self.last_op.take().map(|(_op_pos, op)| op);
                        if visible {
                            self.last_op = Some((self.pos, op));
                        }
                        self.key = Some(key);
                        self.start_pos = self.pos;
                        self.num_ops = 1;
                    }
                    None => {
                        self.key = Some(key);
                        self.start_pos = self.pos;
                        self.num_ops = 1;
                        if visible {
                            self.last_op = Some((self.pos, op));
                        }
                    }
                }
                self.pos += 1;
                if result.is_some() {
                    break;
                }
            } else {
                result = self.last_op.take().map(|(_op_pos, op)| op);
                break;
            }
        }
        result
    }
}