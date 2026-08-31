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

use neva::prelude::*;

/// The report app's document.
const REPORT_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Report</title></head>
<body style="font: 1rem/1.5 var(--font-sans, system-ui)">
  <h1>Report</h1>
  <output id="body">Waiting for data...</output>
  <script>
    window.addEventListener("message", (event) => {
      const text = event.data?.params?.content?.[0]?.text;
      if (text) document.getElementById("body").textContent = text;
    });
  </script>
</body>
</html>
"#;

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
    TextResourceContents::new("ui://report/view", REPORT_HTML)
        .with_mime(APP_MIME_TYPE)
        // `ui_meta` above rides the `resources/list` entry as a static default a
        // host can review at connection time. A block on the content item takes
        // precedence, which is where per-response metadata belongs.
        .with_ui(UiResourceMeta::new().with_prefers_border(false))
}

/// The data half. The id travels in the result, not in the resource URI.
#[tool(descr = "Show a report.", ui = "ui://report/view")]
async fn show_report(id: String) -> String {
    format!("Report {id}: all green.")
}
