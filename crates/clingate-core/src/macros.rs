#[macro_export]
macro_rules! collate_vecs {
    ($($item:expr),* $(,)?) => {{
        let mut capacity = 0;
        $(
            capacity += $crate::macros::CollectIntoVec::size_hint(&$item);
        )*

        let mut vec = Vec::with_capacity(capacity);
        $(
            $crate::macros::CollectIntoVec::collect_into_vec($item, &mut vec);
        )*
        vec
    }};
}

pub trait CollectIntoVec<T> {
    fn size_hint(&self) -> usize;
    fn collect_into_vec(self, vec: &mut Vec<T>);
}

impl<T> CollectIntoVec<T> for Vec<T> {
    fn size_hint(&self) -> usize {
        self.len()
    }

    fn collect_into_vec(self, vec: &mut Vec<T>) {
        vec.extend(self);
    }
}

impl<T> CollectIntoVec<T> for Option<Vec<T>> {
    fn size_hint(&self) -> usize {
        self.as_ref().map_or(0, |v| v.len())
    }

    fn collect_into_vec(self, vec: &mut Vec<T>) {
        if let Some(items) = self {
            vec.extend(items);
        }
    }
}
