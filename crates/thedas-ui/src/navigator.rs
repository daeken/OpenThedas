//! Resource names and image sizing for movie loads from the game resource system.

use std::sync::Arc;

/// Looks a resource up by file name (`mainmenu.gfx`), returning its bytes.
pub type ResourceLookup = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

/// The size an image URL asks for (`img://name.dds?Width=64&Height=64`, as the movies'
/// `MovieUtils.GetImageUrl` builds it): Scaleform scales the image to it.
pub fn requested_size(url: &str) -> Option<(u32, u32)> {
    let query = url.split_once('?')?.1;
    let mut w = None;
    let mut h = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        let v = v
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| *v >= 1.0 && *v <= 4096.0)?;
        match k.to_ascii_lowercase().as_str() {
            "width" => w = Some(v.round() as u32),
            "height" => h = Some(v.round() as u32),
            _ => {}
        }
    }
    Some((w?, h?))
}

/// Candidate resource names for a URL: its last path component, then the same with `.gfx`.
pub fn candidates(url: &str) -> Vec<String> {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.is_empty() {
        return Vec::new();
    }
    let mut out = vec![name.clone()];
    let stem = name.rsplit_once('.').map(|x| x.0).unwrap_or(&name);
    let gfx = format!("{stem}.gfx");
    if gfx != name {
        out.push(gfx);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swf_names_fall_back_to_gfx() {
        assert_eq!(
            candidates("file:///gui/gfxfontlib.swf"),
            vec!["gfxfontlib.swf".to_string(), "gfxfontlib.gfx".to_string()]
        );
        assert_eq!(candidates("Quickbar.GFX?x=1"), vec!["quickbar.gfx"]);
        assert_eq!(
            requested_size("img://a.dds?Width=64&Height=32"),
            Some((64, 32))
        );
        assert_eq!(requested_size("img://a.dds?Async=1"), None);
        assert_eq!(requested_size("img://a.dds"), None);
        assert_eq!(candidates("noext"), vec!["noext", "noext.gfx"]);
        assert!(candidates("file:///gui/").is_empty());
    }
}
