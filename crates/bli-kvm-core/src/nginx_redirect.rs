//! The HTTPS redirect of the board's nginx configuration: the listener on
//! port 80 either redirects (the shipped `return 30x https://`, or the board
//! owner's marker block) or serves. Shared by the board owner, which switches
//! it, and the RAW system update, which carries it into a new image's
//! configuration (code audit of 2026-10-20, V1).

pub const REDIRECT_MARKER_START: &str = "# hardened-blikvm: https redirect";
pub const REDIRECT_MARKER_END: &str = "# hardened-blikvm: end https redirect";

/// Whether the listener on port 80 redirects to HTTPS.
pub fn redirects_port_80(text: &str) -> bool {
    let mut depth = 0_usize;
    let mut block_start = None;
    let mut on_port_80 = false;
    let mut redirect = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') && !trimmed.starts_with(REDIRECT_MARKER_START) {
            continue;
        }
        if depth == 1 && trimmed.starts_with("server") && trimmed.ends_with('{') {
            block_start = Some(depth);
            on_port_80 = false;
            redirect = false;
        }
        if block_start.is_some() {
            if trimmed.starts_with("listen")
                && trimmed
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|address| address == "80" || address.ends_with(":80"))
            {
                on_port_80 = true;
            }
            if trimmed.starts_with(REDIRECT_MARKER_START)
                || (trimmed.starts_with("return 30") && trimmed.contains("https://"))
            {
                redirect = true;
            }
        }
        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
        if block_start.is_some() && depth <= 1 {
            if on_port_80 && redirect {
                return true;
            }
            block_start = None;
        }
    }
    false
}

/// Inserts or removes the owner's redirect inside the first `server` block
/// that listens on port 80.
pub fn render_redirect(text: &str, enabled: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut inserted = false;
    let mut removing = false;
    for line in text.lines() {
        if removing {
            if line.trim().starts_with(REDIRECT_MARKER_END) {
                removing = false;
                continue;
            }
            continue;
        }
        if line.contains(REDIRECT_MARKER_START) {
            if enabled {
                lines.push(line.to_string());
                continue;
            }
            // Drop the marker block entirely when disabling.
            if !line.contains(REDIRECT_MARKER_END) {
                removing = true;
            }
            continue;
        }
        lines.push(line.to_string());
        if enabled
            && !inserted
            && line.trim_start().starts_with("listen")
            && line.contains(":80")
            && line.trim_end().ends_with(';')
        {
            lines.push(format!("        {REDIRECT_MARKER_START}"));
            lines.push("        return 301 https://$host$request_uri;".to_string());
            lines.push(format!("        {REDIRECT_MARKER_END}"));
            inserted = true;
        }
    }
    let mut rendered = lines.join("\n");
    rendered.push('\n');
    rendered
}
