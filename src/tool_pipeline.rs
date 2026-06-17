//! Shared type machinery for composing tool decorators.
//!
//! Both `inject` and `paginate` follow the same decorator pattern:
//! wrap one or more [`ToolDyn`] instances to modify their definitions / calls.
//! This module extracts the generic shape-dispatch (single tool vs. collection)
//! so each decorator only needs to supply a wrapping closure.

use rig::tool::ToolDyn;

/// Marker type for wrapping a single tool.
pub struct Single;
/// Marker type for wrapping a collection of tools.
pub struct Multiple;

/// Apply a transformation to one or more tools.
///
/// The `Shape` parameter disambiguates between the single-tool and
/// collection cases, yielding the correct output type statically.
pub trait ApplyLayer<Shape> {
    type Output;

    fn apply<F>(self, wrap: F) -> Self::Output
    where
        F: Fn(Box<dyn ToolDyn>) -> Box<dyn ToolDyn>;
}

impl<T> ApplyLayer<Single> for T
where
    T: ToolDyn + 'static,
{
    type Output = Box<dyn ToolDyn>;

    fn apply<F>(self, wrap: F) -> Self::Output
    where
        F: Fn(Box<dyn ToolDyn>) -> Box<dyn ToolDyn>,
    {
        wrap(Box::new(self))
    }
}

impl ApplyLayer<Multiple> for Vec<Box<dyn ToolDyn>> {
    type Output = Vec<Box<dyn ToolDyn>>;

    fn apply<F>(self, wrap: F) -> Self::Output
    where
        F: Fn(Box<dyn ToolDyn>) -> Box<dyn ToolDyn>,
    {
        self.into_iter().map(wrap).collect()
    }
}
