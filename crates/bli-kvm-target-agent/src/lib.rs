//! The Hardened BliKVM agent for the target's Windows
//! (`docs/baselines/work-plan-20261014-target-agent.md`).
//!
//! It runs on the computer the board controls, with the user's rights, and
//! tells the board what the board cannot see. This part builds and is tested
//! everywhere; what calls Windows is in the binary.

#![forbid(unsafe_code)]

pub mod client;
pub mod names;
pub mod revert;

/// The agent's version: the workspace's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit the agent was built from, as the build script says it, or
/// `unknown` for a build outside it.
pub fn source_commit() -> &'static str {
    match option_env!("BLI_KVM_SOURCE_COMMIT") {
        Some(commit) if !commit.is_empty() => commit,
        _ => "unknown",
    }
}

/// What the agent calls itself where the user sees it: its name, version and
/// the first twelve characters of its commit.
pub fn title() -> String {
    let commit = source_commit();
    let short = commit.get(..12).unwrap_or(commit);
    format!("Hardened BliKVM agent {VERSION} ({short})")
}

/// The UTF-16 the Windows calls take, ended by a zero, at most `capacity`
/// units with the zero: longer text is cut at a character, not in one.
pub fn wide(text: &str, capacity: usize) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::with_capacity(capacity.min(text.len() + 1));
    let mut buffer = [0_u16; 2];
    for character in text.chars() {
        let units = character.encode_utf16(&mut buffer);
        if out.len() + units.len() + 1 > capacity {
            break;
        }
        out.extend_from_slice(units);
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_the_version_and_the_commit() {
        let title = title();
        assert!(title.starts_with("Hardened BliKVM agent "), "{title}");
        assert!(title.contains(VERSION));
        assert!(title.ends_with(')'));
    }

    #[test]
    fn wide_text_ends_with_a_zero_and_fits() {
        assert_eq!(wide("ab", 8), vec![u16::from(b'a'), u16::from(b'b'), 0]);
        assert_eq!(wide("abcdef", 4), vec![97, 98, 99, 0]);
        // A character outside the basic plane is two units, kept whole.
        let cut = wide("a\u{1F600}b", 3);
        assert_eq!(cut, vec![97, 0]);
        assert_eq!(wide("", 1), vec![0]);
    }
}
