//! Greeting banner for the MCP server startup

const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// Renders a startup greeting banner to stderr
#[derive(Debug)]
pub(super) struct Greeter<'a> {
    pub(super) server_name: &'a str,
    pub(super) server_version: &'a str,
    /// Always `env!("CARGO_PKG_VERSION")` from the neva crate
    pub(super) neva_version: &'a str,
    pub(super) transport_label: &'a str,
    pub(super) tools: &'a [String],
    pub(super) prompts: &'a [String],
    pub(super) resource_templates: &'a [String],
    /// Set by caller: `std::env::var_os("NO_COLOR").is_none()`
    pub(super) use_color: bool,
}

impl<'a> Greeter<'a> {
    /// Renders the full banner to a `String`. Used by `print()` and tests.
    pub(super) fn render(&self) -> String {
        // Build the box content lines
        let server_line = format!("{} v{}", self.server_name, self.server_version);
        let neva_line = format!("powered by neva v{}", self.neva_version);
        let transport_line = format!("Transport: {}", self.transport_label);

        // Dynamic width based on longest content line (chars().count() handles non-ASCII)
        let text_width = [
            server_line.chars().count(),
            neva_line.chars().count(),
            transport_line.chars().count(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);

        let inner_width = text_width + 4; // 2 leading + 2 trailing spaces

        let mut out = String::new();

        // ╔══...══╗
        out.push('╔');
        out.push_str(&"═".repeat(inner_width));
        out.push_str("╗\n");

        // Content lines: server name and neva version
        for text in &[&server_line, &neva_line] {
            let padding = inner_width - 2 - text.chars().count();
            out.push_str("║  ");
            out.push_str(text);
            out.push_str(&" ".repeat(padding));
            out.push_str("║\n");
        }

        // Blank separator line
        out.push('║');
        out.push_str(&" ".repeat(inner_width));
        out.push_str("║\n");

        // Transport line
        let padding = inner_width - 2 - transport_line.chars().count();
        out.push_str("║  ");
        out.push_str(&transport_line);
        out.push_str(&" ".repeat(padding));
        out.push_str("║\n");

        // ╚══...══╝
        out.push('╚');
        out.push_str(&"═".repeat(inner_width));
        out.push_str("╝\n");

        // Capability sections (outside the box)
        let sections: &[(&str, &str, &[String])] = &[
            (CYAN, "Tools", self.tools),
            (GREEN, "Prompts", self.prompts),
            (YELLOW, "Resource Templates", self.resource_templates),
        ];

        for (color, header, items) in sections {
            if items.is_empty() {
                continue;
            }
            out.push('\n');
            if self.use_color {
                out.push_str(color);
                out.push_str(header);
                out.push_str(RESET);
            } else {
                out.push_str(header);
            }
            out.push('\n');
            for item in *items {
                out.push_str("  \u{2022} ");
                out.push_str(item);
                out.push('\n');
            }
        }

        out
    }

    /// Writes the banner to stderr; write errors are silently discarded.
    pub(super) fn print(&self) {
        eprint!("{}", self.render());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_greeter<'a>(
        server_name: &'a str,
        tools: &'a [String],
        prompts: &'a [String],
        resource_templates: &'a [String],
        use_color: bool,
    ) -> Greeter<'a> {
        Greeter {
            server_name,
            server_version: "1.0.0",
            neva_version: "0.2.1",
            transport_label: "stdio",
            tools,
            prompts,
            resource_templates,
            use_color,
        }
    }

    #[test]
    fn it_renders_box_wide_enough_for_longest_line() {
        // Server line will be the longest: "<name> v1.0.0"
        let long_name = "My Very Long Server Name That Is Long";
        let greeter = make_greeter(long_name, &[], &[], &[], false);
        let output = greeter.render();

        let server_line_len = format!("{} v1.0.0", long_name).chars().count();
        let inner_width = server_line_len + 4;
        let expected_top = format!("╔{}╗", "═".repeat(inner_width));
        assert!(
            output.contains(&expected_top),
            "Expected top border of width {inner_width} not found in:\n{output}"
        );
    }

    #[test]
    fn it_omits_empty_sections() {
        let tools = vec!["hello".to_string()];
        let greeter = make_greeter("Server", &tools, &[], &[], false);
        let output = greeter.render();

        assert!(output.contains("Tools"), "Tools section should be present");
        assert!(
            !output.contains("Prompts"),
            "Prompts section should be absent"
        );
        assert!(
            !output.contains("Resource Templates"),
            "Resource Templates section should be absent"
        );
    }

    #[test]
    fn it_omits_ansi_when_use_color_false() {
        let tools = vec!["hello".to_string()];
        let greeter = make_greeter("Server", &tools, &[], &[], false);
        let output = greeter.render();
        assert!(
            !output.contains("\x1b["),
            "No ANSI codes expected when use_color=false, got:\n{output}"
        );
    }

    #[test]
    fn it_includes_ansi_when_use_color_true() {
        let tools = vec!["hello".to_string()];
        let greeter = make_greeter("Server", &tools, &[], &[], true);
        let output = greeter.render();
        assert!(
            output.contains("\x1b[36m"),
            "Expected cyan ANSI code for Tools header, got:\n{output}"
        );
    }

    #[test]
    fn it_renders_only_box_when_all_sections_empty() {
        let greeter = make_greeter("Server", &[], &[], &[], false);
        let output = greeter.render();

        // Box must be present
        assert!(output.contains("╔"), "Box top-left corner missing");
        assert!(output.contains("╝"), "Box bottom-right corner missing");

        // No capability headers
        assert!(!output.contains("Tools"));
        assert!(!output.contains("Prompts"));
        assert!(!output.contains("Resource Templates"));

        // No trailing blank line after the box close (output ends at ╝\n)
        assert!(
            output.trim_end().ends_with('╝'),
            "Output should end with box bottom"
        );
    }
}
