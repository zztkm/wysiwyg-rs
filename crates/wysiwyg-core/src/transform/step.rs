use std::sync::Arc;

use crate::model::node::Node;

use super::{
    mark_step::{AddMarkStep, RemoveMarkStep, ReplaceAroundStep},
    replace_step::ReplaceStep,
    step_map::StepMap,
};

/// Error produced when a step cannot be applied to a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepError {
    InvalidRange { from: usize, to: usize },
    InvalidPosition(usize),
    InvalidContent(String),
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepError::InvalidRange { from, to } => {
                write!(f, "Invalid range: from={from} > to={to}")
            }
            StepError::InvalidPosition(p) => write!(f, "Position {p} is out of range"),
            StepError::InvalidContent(msg) => write!(f, "Invalid content: {msg}"),
        }
    }
}

impl std::error::Error for StepError {}

/// The outcome of applying a step: new document + position map.
pub type StepResult = Result<(Arc<Node>, StepMap), StepError>;

/// An atomic document transformation.
///
/// This is a closed enum — the set of step kinds is intentionally fixed.
/// New editing behaviours compose these primitives rather than adding variants.
#[derive(Debug, Clone)]
pub enum Step {
    Replace(ReplaceStep),
    AddMark(AddMarkStep),
    RemoveMark(RemoveMarkStep),
    ReplaceAround(ReplaceAroundStep),
}

impl Step {
    /// Apply the step to `doc`, producing a new document and a `StepMap`.
    pub fn apply(&self, doc: &Arc<Node>) -> StepResult {
        match self {
            Step::Replace(s) => s.apply(doc),
            Step::AddMark(s) => s.apply(doc),
            Step::RemoveMark(s) => s.apply(doc),
            Step::ReplaceAround(s) => s.apply(doc),
        }
    }

    /// Create the inverse of this step (for undo/redo).
    ///
    /// `doc` must be the document state **before** the step was applied.
    pub fn invert(&self, doc: &Arc<Node>) -> Step {
        match self {
            Step::Replace(s) => s.invert(doc),
            Step::AddMark(s) => s.invert(doc),
            Step::RemoveMark(s) => s.invert(doc),
            Step::ReplaceAround(s) => s.invert(doc),
        }
    }

    /// Map this step's positions through a `Mapping`, producing a step
    /// adjusted for earlier changes.  Returns `None` if the step becomes
    /// a no-op or invalid after mapping.
    pub fn map(&self, mapping: &super::step_map::Mapping) -> Option<Step> {
        match self {
            Step::Replace(s) => s.map(mapping),
            Step::AddMark(s) => s.map(mapping),
            Step::RemoveMark(s) => s.map(mapping),
            Step::ReplaceAround(s) => s.map(mapping),
        }
    }

    /// The `StepMap` that this step would produce (without applying it).
    ///
    /// Useful for building position mappings before applying.
    pub fn get_map(&self) -> StepMap {
        match self {
            Step::Replace(s) => s.get_map(),
            Step::AddMark(s) => s.get_map(),
            Step::RemoveMark(s) => s.get_map(),
            Step::ReplaceAround(s) => s.get_map(),
        }
    }
}
