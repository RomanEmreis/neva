//! The generated-HTML path, and what `_meta.ui.resourceUri` may point at.
//!
//! `add_ui_resource` covers a fixed document. When the markup is computed --
//! read from disk, templated, assembled at read time -- register it the way any
//! other resource is registered: the `ui://` scheme is what marks it as an app,
//! and the macro takes it from there, filling in the
//! `text/html;profile=mcp-app` MIME type and checking the `ui_meta` block at
//! compile time.
//!
//! What the URI must *not* be is a template. A host fetches
//! `_meta.ui.resourceUri` verbatim -- nothing substitutes a tool argument into
//! it -- so `ui://report/{id}` would be read as a literal and render a report
//! for `{id}`. That is not a gap in the specification, it is its design: the
//! document is the static, cacheable, reviewable half, and the data arrives in
//! the iframe as the tool's result. One document, every report.

use crate::view;
use neva::prelude::*;

/// One document for every report.
#[resource(
    uri = "ui://report/view",
    title = "Report",
    descr = "Renders whichever report the tool just returned",
    ui_meta = r#"{
        "csp": { "resourceDomains": ["https://cdn.jsdelivr.net"] },
        "prefersBorder": false
    }"#
)]
async fn report_view() -> TextResourceContents {
    // Neither `_meta.ui` nor a MIME type here. The server supplies both for a
    // `ui://` read: the attribute's block falls back onto the content item --
    // the only place the tool-driven flow looks -- and the app MIME type is
    // stamped on, since the spec allows a `ui://` resource no other one and
    // `TextResourceContents::new` would otherwise ship `text/plain`.
    //
    // Return a block of your own -- `TextResourceContents::with_ui(..)` -- when
    // it varies per response; that replaces the attribute's *whole* block rather
    // than merging into it, which is the precedence the specification gives a
    // host.
    TextResourceContents::new(
        "ui://report/view",
        view::document(
            "Report",
            r#"  <h1>Report</h1>
  <output id="out">Waiting for data...</output>"#,
        ),
    )
}

/// The data half. The id travels in the result, not in the resource URI.
#[tool(descr = "Show a report.", ui = "ui://report/view")]
async fn show_report(id: String) -> String {
    format!("Report {id}: all green.")
}
