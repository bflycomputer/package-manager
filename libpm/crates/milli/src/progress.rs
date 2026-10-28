use std::marker::PhantomData;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

pub trait Step: 'static + Send + Sync {}

#[derive(Clone, Default)]
pub struct Progress;

impl Progress {
    pub fn update_progress<P: Step>(&self, _step: P) {}

    pub fn update_progress_scoped<P: Step + Copy>(
        &self,
        _step: P,
    ) -> ScopedProgressStep<'_, P> {
        ScopedProgressStep { marker: PhantomData }
    }
}

pub trait NamedStep: 'static + Send + Sync + Default {}

pub struct AtomicSubStep<Name: NamedStep> {
    marker: PhantomData<Name>,
}

impl<Name: NamedStep> AtomicSubStep<Name> {
    pub fn new(_total: u32) -> (Arc<AtomicU32>, Self) {
        (Arc::new(AtomicU32::new(0)), Self { marker: PhantomData })
    }
}

impl<Name: NamedStep> Step for AtomicSubStep<Name> {}

#[macro_export]
macro_rules! make_enum_progress {
    ($visibility:vis enum $name:ident { $($variant:ident,)+ }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $visibility enum $name {
            $($variant),+
        }

        impl $crate::progress::Step for $name {}
    };
}

#[macro_export]
macro_rules! make_atomic_progress {
    ($struct_name:ident alias $atomic_struct_name:ident => $step_name:literal) => {
        #[derive(Default, Debug, Clone, Copy)]
        pub struct $struct_name;
        impl NamedStep for $struct_name {}
        pub type $atomic_struct_name = AtomicSubStep<$struct_name>;
    };
}

make_atomic_progress!(Document alias AtomicDocumentStep => "document");

make_enum_progress! {
    pub enum MergingWordCache {
        WordDocids,
        WordFieldIdDocids,
        WordPositionDocids,
        FieldIdWordCountDocids,
    }
}

impl steppe::Progress for Progress {
    fn update(&self, _step: impl steppe::Step) {}
}

pub struct ScopedProgressStep<'a, P: Step + Copy> {
    marker: PhantomData<(&'a Progress, P)>,
}
