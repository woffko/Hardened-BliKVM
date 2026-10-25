//! Where the captured screen lies in the target's desktop
//! (`docs/design/screen-area-v1.md`).
//!
//! A target that extends its desktop over more than one monitor spreads the
//! absolute mouse's range over all of them, and the board captures one. The
//! page maps the picture into the part of the range that is that monitor; the
//! board keeps the layout for every browser, in the server's state directory,
//! which a RAW update carries. No layout means the screen is the whole
//! desktop.

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

/// The largest size or position, in the target's pixels.
pub const MAX_DESKTOP_PIXELS: u32 = 32_767;
pub const SCREEN_AREA_FILE: &str = "screen-area.json";
const MAX_FILE_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenPlace {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The whole desktop, and the captured screen in it from its top left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenArea {
    pub desktop: DesktopSize,
    pub screen: ScreenPlace,
}

impl ScreenArea {
    /// Whole pixels within bounds, the screen inside the desktop.
    pub fn is_valid(&self) -> bool {
        let size = |value: u32| (1..=MAX_DESKTOP_PIXELS).contains(&value);
        size(self.desktop.width)
            && size(self.desktop.height)
            && size(self.screen.width)
            && size(self.screen.height)
            && self.screen.x <= MAX_DESKTOP_PIXELS
            && self.screen.y <= MAX_DESKTOP_PIXELS
            && self.screen.x + self.screen.width <= self.desktop.width
            && self.screen.y + self.screen.height <= self.desktop.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenAreaView {
    /// `null`: the screen is the whole desktop.
    pub area: Option<ScreenArea>,
}

/// The layout the board keeps; a missing, oversized or malformed file, or one
/// that is not a layout, is none.
pub fn load(state_dir: &std::path::Path) -> Option<ScreenArea> {
    std::fs::read(state_dir.join(SCREEN_AREA_FILE))
        .ok()
        .filter(|bytes| bytes.len() <= MAX_FILE_BYTES)
        .and_then(|bytes| serde_json::from_slice::<ScreenAreaView>(&bytes).ok())
        .and_then(|view| view.area)
        .filter(ScreenArea::is_valid)
}

fn store(state_dir: &std::path::Path, view: &ScreenAreaView) -> std::io::Result<()> {
    crate::state_file::replace(
        state_dir,
        ".screen-area-",
        SCREEN_AREA_FILE,
        &serde_json::to_vec(view).map_err(std::io::Error::other)?,
    )
}

pub(crate) async fn get_screen_area(
    State(state): State<AppState>,
) -> Json<ApiResponse<ScreenAreaView>> {
    Json(ApiResponse::ok(ScreenAreaView {
        area: load(&state.options.state_dir),
    }))
}

pub(crate) async fn set_screen_area(
    State(state): State<AppState>,
    Json(body): Json<ScreenAreaView>,
) -> AppResult<Json<ApiResponse<ScreenAreaView>>> {
    if body.area.is_some_and(|area| !area.is_valid()) {
        return Err(AppError::BadRequest(
            "the screen does not lie inside the desktop".into(),
        ));
    }
    if let Err(error) = store(&state.options.state_dir, &body) {
        eprintln!("screen area: the layout could not be saved: {error}");
        return Err(AppError::ServiceUnavailable(
            "the screen area could not be saved".into(),
        ));
    }
    Ok(Json(ApiResponse::ok(ScreenAreaView {
        area: load(&state.options.state_dir),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: ScreenArea = ScreenArea {
        desktop: DesktopSize {
            width: 5120,
            height: 1440,
        },
        screen: ScreenPlace {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        },
    };

    #[test]
    fn a_layout_has_its_screen_inside_its_desktop() {
        assert!(LEFT.is_valid());
        for bad in [
            ScreenArea {
                screen: ScreenPlace {
                    x: 2561,
                    ..LEFT.screen
                },
                ..LEFT
            },
            ScreenArea {
                screen: ScreenPlace {
                    width: 0,
                    ..LEFT.screen
                },
                ..LEFT
            },
            ScreenArea {
                desktop: DesktopSize {
                    width: 40_000,
                    height: 1440,
                },
                ..LEFT
            },
            ScreenArea {
                screen: ScreenPlace {
                    x: u32::MAX,
                    ..LEFT.screen
                },
                ..LEFT
            },
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
    }

    #[test]
    fn the_file_is_read_only_when_it_holds_a_layout() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(load(directory.path()), None);
        store(directory.path(), &ScreenAreaView { area: Some(LEFT) }).unwrap();
        assert_eq!(load(directory.path()), Some(LEFT));
        store(directory.path(), &ScreenAreaView { area: None }).unwrap();
        assert_eq!(load(directory.path()), None);
        let path = directory.path().join(SCREEN_AREA_FILE);
        std::fs::write(
            &path,
            r#"{"area":{"desktop":{"width":100,"height":100},"screen":{"x":90,"y":0,"width":20,"height":100}}}"#,
        )
        .unwrap();
        assert_eq!(load(directory.path()), None);
        std::fs::write(&path, " ".repeat(600)).unwrap();
        assert_eq!(load(directory.path()), None);
    }
}
