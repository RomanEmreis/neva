//! The clock app's document.

use crate::view;

/// The clock's markup. The handshake and the result plumbing come from
/// [`view::document`].
pub(crate) fn document() -> String {
    view::document(
        "Clock",
        r#"  <h1 style="font-size: 3rem; text-align: center"><output id="out">...</output></h1>"#,
    )
}
