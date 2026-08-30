//! The app itself: an HTML document the host renders in a sandboxed iframe.
//!
//! It talks to nobody but its own host. The `message` event carries the
//! `tools/call` result the host pushes in; everything else here is ordinary
//! page code. A real app would use the `@modelcontextprotocol/ext-apps` browser
//! SDK instead of raw `postMessage`.

/// The clock app's document.
pub(crate) const CLOCK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Clock</title></head>
<body style="font: 3rem/1.4 var(--font-sans, system-ui); text-align: center">
  <output id="now">...</output>
  <script>
    window.addEventListener("message", (event) => {
      const text = event.data?.params?.content?.[0]?.text;
      if (text) document.getElementById("now").textContent = text;
    });
    window.parent.postMessage(
      { jsonrpc: "2.0", method: "ui/notifications/initialized", params: {} },
      "*",
    );
  </script>
</body>
</html>
"#;
