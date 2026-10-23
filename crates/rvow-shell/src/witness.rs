//! The name of the record of what the user made, and the text of a denial for
//! an agent that reaches for it. The record itself is rvow-core's, since it
//! reads the store.

pub use rvow_base::layout::WITNESS_FILE;

/// The text of a denial for an agent that reaches for the record.
pub fn reason() -> String {
    format!(
        "{WITNESS_FILE} holds what the user made and is written only by the harness; ask the user to run /rvow-accept <id> if an accept is missing"
    )
}
