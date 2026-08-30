//! The generated-HTML path: an app whose document is computed per request.
//!
//! `add_ui_resource` covers a fixed document. When the HTML depends on the URI,
//! register it the way any other resource is registered -- the `ui://` scheme is
//! what marks it as an app, and the macro takes it from there: it fills in the
//! `text/html;profile=mcp-app` MIME type and checks the `ui_meta` block's keys
//! at compile time.

use neva::prelude::*;

/// A report, rendered as an app.
#[resource(
    uri = "ui://report/{id}",
    title = "Report",
    descr = "A report, rendered as an app",
    ui_meta = r#"{
        "csp": { "resourceDomains": ["https://cdn.jsdelivr.net"] },
        "prefersBorder": false
    }"#
)]
async fn report(id: String) -> TextResourceContents {
    TextResourceContents::new(
        format!("ui://report/{id}"),
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>Report {id}</title></head><body><h1>Report {id}</h1></body></html>"
        ),
    )
    .with_mime(APP_MIME_TYPE)
}

/// Opens the report app for `id`.
#[tool(descr = "Show a report.", ui = "ui://report/{id}")]
async fn show_report(id: String) -> String {
    format!("Report {id} is ready.")
}
