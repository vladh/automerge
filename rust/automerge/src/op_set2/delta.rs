use super::{
    ColExport, ColumnCursor, Encoder, PackError, Packable, RleCursor, RleState, Run, Slab,
    SlabWriter,
};

type SubCursor<const B: usize> = RleCursor<B, i64>;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DeltaCursorInternal<const B: usize> {
    abs: i64,
    rle: SubCursor<B>,
}

pub(crate) type DeltaCursor = DeltaCursorInternal<1024>;

#[derive(Debug, Default, Clone)]
pub(crate) struct DeltaState<'a> {
    abs: i64,
    rle: RleState<'a, i64>,
}

impl<'a> DeltaState<'a> {
    fn new(abs: i64) -> Self {
        DeltaState {
            abs,
            rle: RleState::Empty,
        }
    }
}

impl<const B: usize> ColumnCursor for DeltaCursorInternal<B> {
    type Item = i64;
    type State<'a> = DeltaState<'a>;
    type PostState<'a> = Option<Run<'a, i64>>;
    type Export = Option<i64>;

    fn finish<'a>(
        slab: &'a Slab,
        out: &mut SlabWriter<'a>,
        mut state: Self::State<'a>,
        post: Self::PostState<'a>,
        cursor: Self,
    ) {
        match post {
            Some(Run {
                count: 1,
                value: Some(_),
            }) => {
                //let delta = cursor.abs - state.abs;
                Self::append(&mut state, out, Some(cursor.abs));
                SubCursor::<B>::finish(slab, out, RleState::Empty, None, cursor.rle);
            }
            Some(Run {
                count,
                value: Some(v),
            }) => {
                Self::append(&mut state, out, Some(cursor.abs - (count as i64 - 1) * v));
                let next_post = Some(Run::new(count - 1, Some(v)));
                SubCursor::<B>::finish(slab, out, state.rle, next_post, cursor.rle);
            }
            Some(Run { count, value: None }) => {
                let next_state = DeltaState::new(state.abs);
                SubCursor::<B>::flush_state(out, state.rle);
                SubCursor::<B>::flush_run(out, count, None);
                Self::finish(slab, out, next_state, None, cursor);
            }
            None => {
                if let Some((run, next_cursor)) = cursor.next(slab.as_ref()) {
                    match run {
                        Run { count, value: None } => {
                            let next_state = DeltaState::new(state.abs);
                            SubCursor::<B>::flush_state(out, state.rle);
                            SubCursor::<B>::flush_run(out, count, None);
                            Self::finish(slab, out, next_state, None, next_cursor);
                        }
                        Run {
                            count: 1,
                            value: Some(_),
                        } => {
                            Self::append(&mut state, out, Some(next_cursor.abs));
                            SubCursor::<B>::finish(slab, out, state.rle, None, next_cursor.rle);
                        }
                        run => {
                            let run = Run::new(run.count - 1, run.value);
                            Self::append(&mut state, out, Some(next_cursor.abs - run.delta()));
                            SubCursor::<B>::finish(
                                slab,
                                out,
                                state.rle,
                                Some(run),
                                next_cursor.rle,
                            );
                        }
                    }
                } else {
                    SubCursor::<B>::flush_state(out, state.rle);
                }
            }
        }
    }

    fn pop<'a>(
        &self,
        mut run: Run<'a, Self::Item>,
    ) -> (
        Option<<Self::Item as Packable>::Unpacked<'a>>,
        Option<Run<'a, Self::Item>>,
    ) {
        run.count -= 1;
        let value = run.value.map(|_| self.abs - run.delta());
        if run.count > 0 {
            (value, Some(run))
        } else {
            (value, None)
        }
    }

    fn flush_state<'a>(out: &mut SlabWriter<'a>, state: Self::State<'a>) {
        SubCursor::<B>::flush_state(out, state.rle)
    }

    fn copy_between<'a>(
        slab: &'a Slab,
        out: &mut SlabWriter<'a>,
        c0: Self,
        c1: Self,
        run: Run<'a, i64>,
        size: usize,
    ) -> Self::State<'a> {
        let rle = SubCursor::<B>::copy_between(slab, out, c0.rle, c1.rle, run, size);
        let mut rle = RleState::Empty;
        SubCursor::<B>::append_chunk(&mut rle, out, run);
        DeltaState { abs: c1.abs, rle }
    }

    fn append<'a>(state: &mut Self::State<'a>, slab: &mut SlabWriter<'a>, item: Option<i64>) {
        let value = item.map(|i| i - state.abs);
        Self::append_chunk(state, slab, Run { count: 1, value })
    }

    fn append_chunk<'a>(state: &mut Self::State<'a>, slab: &mut SlabWriter<'a>, run: Run<'a, i64>) {
        state.abs += run.delta();
        SubCursor::<B>::append_chunk(&mut state.rle, slab, run);
    }

    fn encode<'a>(index: usize, slab: &'a Slab) -> Encoder<'a, Self> {
        let (run, cursor) = Self::seek(index, slab.as_ref());

        let last_run_count = run.as_ref().map(|r| r.count).unwrap_or(0);

        let (rle, post) = SubCursor::<B>::encode_inner(&cursor.rle, run, index, slab);

        let abs_delta = post.as_ref().map(|run| run.delta()).unwrap_or(0);
        let abs = cursor.abs - abs_delta;
        let state = DeltaState { abs, rle };

        let current = cursor.rle.start_copy(slab, last_run_count);

        Encoder {
            slab,
            current,
            post,
            state,
            cursor,
        }
    }

    fn export_item(item: Option<i64>) -> Option<i64> {
        item
    }

    fn export(data: &[u8]) -> Vec<ColExport<i64>> {
        SubCursor::<B>::export(data)
    }

    fn try_next<'a>(
        &self,
        slab: &'a [u8],
    ) -> Result<Option<(Run<'a, Self::Item>, Self)>, PackError> {
        if let Some((run, rle)) = self.rle.try_next(slab)? {
            let delta = run.delta();
            let abs = self.abs.saturating_add(delta);
            Ok(Some((run, Self { abs, rle })))
        } else {
            Ok(None)
        }
    }

    fn index(&self) -> usize {
        self.rle.index()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::columns::{ColExport, ColumnData};
    use super::*;

    #[test]
    fn column_data_bool_split_merge_semantics() {
        // lit run spanning multiple slabs
        let mut col1: ColumnData<DeltaCursorInternal<5>> = ColumnData::new();
        col1.splice(0, vec![1, 10, 2, 11, 4, 27, 19, 3, 21, 14, 2, 8]);
        assert_eq!(
            col1.export(),
            vec![
                vec![ColExport::litrun(vec![1, 9, -8, 9])],
                vec![ColExport::litrun(vec![-7, 23, -8, -16])],
                vec![ColExport::litrun(vec![18, -7, -12, 6])],
            ]
        );
        let mut out = Vec::new();
        col1.write(&mut out);
        assert_eq!(
            out,
            vec![116, 1, 9, 120, 9, 121, 23, 120, 112, 18, 121, 116, 6]
        );

        // lit run capped by runs
        let mut col2: ColumnData<DeltaCursorInternal<5>> = ColumnData::new();
        col2.splice(0, vec![1, 2, 10, 11, 4, 27, 19, 3, 21, 14, 15, 16]);
        assert_eq!(
            col2.export(),
            vec![
                vec![ColExport::run(2, 1), ColExport::litrun(vec![8, 1])],
                vec![ColExport::litrun(vec![-7, 23, -8, -16])],
                vec![ColExport::litrun(vec![18, -7]), ColExport::run(2, 1)],
            ]
        );
        let mut out = Vec::new();
        col2.write(&mut out);

        assert_eq!(out, vec![2, 1, 120, 8, 1, 121, 23, 120, 112, 18, 121, 2, 1]);

        // lit run capped by runs
        let mut col3: ColumnData<DeltaCursorInternal<5>> = ColumnData::new();
        col3.splice(0, vec![1, 10, 5, 6, 7, 9, 11, 20, 25, 19, 10, 9, 19, 29]);
        assert_eq!(
            col3.export(),
            vec![
                vec![ColExport::litrun(vec![1, 9, -5]), ColExport::run(2, 1),],
                vec![ColExport::run(2, 2), ColExport::litrun(vec![9, 5]),],
                vec![ColExport::litrun(vec![-6, -9, -1]), ColExport::run(2, 10)],
            ]
        );
        let mut out = Vec::new();
        col3.write(&mut out);
        assert_eq!(
            out,
            vec![125, 1, 9, 123, 2, 1, 2, 2, 123, 9, 5, 122, 119, 127, 2, 10]
        );

        // lit run capped by runs
        let mut col4: ColumnData<DeltaCursorInternal<5>> = ColumnData::new();
        col4.splice(
            0,
            vec![
                1, 2, 4, 6, 9, 12, 16, 20, 25, 30, 36, 42, 49, 56, 64, 72, 81, 90,
            ],
        );
        assert_eq!(
            col4.export(),
            vec![
                vec![
                    ColExport::run(2, 1),
                    ColExport::run(2, 2),
                    ColExport::run(2, 3),
                ],
                vec![
                    ColExport::run(2, 4),
                    ColExport::run(2, 5),
                    ColExport::run(2, 6),
                ],
                vec![
                    ColExport::run(2, 7),
                    ColExport::run(2, 8),
                    ColExport::run(2, 9),
                ],
            ]
        );
        let mut out = Vec::new();
        col4.write(&mut out);
        assert_eq!(
            out,
            vec![2, 1, 2, 2, 2, 3, 2, 4, 2, 5, 2, 6, 2, 7, 2, 8, 2, 9]
        );

        // empty data
        let mut col5: ColumnData<DeltaCursorInternal<5>> = ColumnData::new();
        assert_eq!(col5.export(), vec![vec![]]);
        let mut out = Vec::new();
        col5.write(&mut out);
        assert_eq!(out, Vec::<u8>::new());
    }
}