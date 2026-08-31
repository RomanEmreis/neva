//! The View side of an MCP App: the handshake every document owes its host.
//!
//! A View is an MCP client speaking JSON-RPC over `postMessage`, and it opens
//! the way any client does. The order is not decoration: the host **MUST NOT**
//! send a request or a notification to a View before it has seen
//! `ui/notifications/initialized`, and that notification only follows a
//! completed `ui/initialize` exchange. Skip either and a conforming host holds
//! the tool result back, leaving the document sitting on its placeholder.
//!
//! Real apps use the browser SDK
//! ([`@modelcontextprotocol/ext-apps`](https://github.com/modelcontextprotocol/ext-apps)),
//! which does all of this and hands you `ontoolresult`, `callServerTool` and
//! `getHostContext`. This is the same thing written out, so the shape is visible.

/// The protocol version the View announces. Tracks the MCP Apps specification,
/// not the MCP one.
const UI_PROTOCOL_VERSION: &str = "2026-01-26";

/// Wraps `body` in a document that completes the handshake and then renders
/// whatever the tool returned into `#out`.
pub(crate) fn document(name: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>{name}</title></head>
<body style="font: 1rem/1.5 var(--font-sans, system-ui)">
{body}
<script>
  let nextId = 1;

  function request(method, params) {{
    const id = nextId++;
    return new Promise((resolve, reject) => {{
      window.addEventListener("message", function listener(event) {{
        const msg = event.data;
        if (msg?.id !== id) return;
        window.removeEventListener("message", listener);
        if (msg.error) reject(new Error(msg.error.message));
        else resolve(msg.result);
      }});
      window.parent.postMessage({{ jsonrpc: "2.0", id, method, params }}, "*");
    }});
  }}

  function notify(method, params) {{
    window.parent.postMessage({{ jsonrpc: "2.0", method, params }}, "*");
  }}

  function on(method, handler) {{
    window.addEventListener("message", (event) => {{
      if (event.data?.method === method) handler(event.data.params);
    }});
  }}

  // Registered before the handshake finishes: the host may send the result the
  // moment it sees `initialized`, and a listener added after that would miss it.
  on("ui/notifications/tool-result", (result) => {{
    const text = result?.content?.[0]?.text;
    if (text) document.getElementById("out").textContent = text;
  }});

  (async () => {{
    // `appCapabilities` is required. Declaring `availableDisplayModes` keeps the
    // host from switching this document into a mode it cannot lay out.
    await request("ui/initialize", {{
      appInfo: {{ name: "{name}", version: "0.1.0" }},
      appCapabilities: {{ availableDisplayModes: ["inline"] }},
      protocolVersion: "{UI_PROTOCOL_VERSION}",
    }});
    notify("ui/notifications/initialized");
  }})();
</script>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handshake the host waits on, in the order it waits for it.
    #[test]
    fn the_document_opens_with_the_full_handshake() {
        let doc = document("Clock", "<output id=\"out\"></output>");

        let initialize = doc.find("ui/initialize").expect("the request");
        let initialized = doc
            .find("ui/notifications/initialized")
            .expect("the notification");
        assert!(
            initialize < initialized,
            "`initialized` only follows a completed `ui/initialize`"
        );

        // Required by the spec; a host has nothing to negotiate without them.
        assert!(doc.contains("appInfo"));
        assert!(doc.contains("appCapabilities"));
        assert!(doc.contains(UI_PROTOCOL_VERSION));

        // Registered before the handshake, or the first result is lost.
        let listener = doc
            .find("ui/notifications/tool-result")
            .expect("a listener");
        assert!(listener < initialize);
    }

    #[test]
    fn braces_survive_the_format_escaping() {
        let doc = document("Clock", "");
        let opens = doc.matches('{').count();
        let closes = doc.matches('}').count();

        assert_eq!(opens, closes, "unbalanced braces in the generated script");
        assert!(
            !doc.contains("{{"),
            "an unescaped `{{{{` reached the output"
        );
    }

    /// Writes the rendered document for an external syntax check.
    #[test]
    #[ignore = "manual: dumps the document for `node --check`"]
    fn dump() {
        std::fs::write("/tmp/mcp-app-view.html", document("Clock", "")).unwrap();
    }
}
