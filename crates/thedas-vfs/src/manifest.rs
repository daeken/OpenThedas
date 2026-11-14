//! Add-in manifest metadata. Only our own descriptions of the executable belong here.

use crate::install::AddIn;
use roxmltree::Node;

#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub title: String,
    pub description: String,
    pub publisher: String,
    /// The add-in's release label, not a minimum game version.
    pub version: String,
    pub game_version: String,
    pub kind: Option<i64>,
    pub prerequisites: Vec<Prerequisite>,
}

#[derive(Clone, Debug)]
pub struct Prerequisite {
    pub product_id: i64,
    /// This is the add-in UID used by the enable operation, not a filesystem path.
    pub uid: String,
    pub title: String,
    pub strref: Option<u32>,
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(name))
}

fn text(node: Node<'_, '_>, name: &str) -> String {
    let Some(n) = child(node, name) else {
        return String::new();
    };
    // The engine currently uses English resources. Keep the manifest's fallback when it
    // has no English translation; XML entities and CDATA are decoded by the XML reader.
    child(n, "en-us")
        .and_then(|n| n.text())
        .or_else(|| n.attribute("DefaultText"))
        .or_else(|| n.text())
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub(crate) fn metadata(node: Node<'_, '_>) -> Metadata {
    let format_one = node.attribute("Format").is_some_and(|v| v != "0");
    Metadata {
        title: text(node, "Title"),
        description: text(node, "Description"),
        publisher: text(node, "Publisher"),
        version: text(node, "Version"),
        game_version: text(node, "GameVersion"),
        kind: text(node, "Type").parse().ok(),
        prerequisites: child(node, "PrereqList")
            .into_iter()
            .flat_map(|n| n.children())
            .filter(|n| n.has_tag_name("PrereqItem"))
            .filter_map(|n| {
                // 0x5f3690/0x5f38c0 accept these three attributes (plus strref for
                // format 1). Other/incomplete records are not added to the vector.
                if n.attributes().any(|a| {
                    !matches!(
                        a.name(),
                        "ProductID" | "microContentID" | "Title" | "strref"
                    )
                }) {
                    return None;
                }
                if format_one && n.attribute("strref").is_none() {
                    return None;
                }
                Some(Prerequisite {
                    product_id: n.attribute("ProductID")?.parse().ok()?,
                    uid: n.attribute("microContentID")?.to_string(),
                    title: n.attribute("Title")?.to_string(),
                    strref: n.attribute("strref").and_then(|s| s.parse().ok()),
                })
            })
            .collect(),
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<String, String> {
    let little = bytes.starts_with(&[0xff, 0xfe]);
    if little || bytes.starts_with(&[0xfe, 0xff]) {
        let bytes = &bytes[2..];
        if bytes.len() % 2 != 0 {
            return Err("odd UTF-16 byte count".into());
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|b| {
                if little {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|e| e.to_string())
    } else {
        std::str::from_utf8(bytes)
            .map(|s| s.trim_start_matches('\u{feff}').to_string())
            .map_err(|e| e.to_string())
    }
}

pub(crate) fn items(text: &str) -> Result<Vec<AddIn>, String> {
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("AddInItem")) {
        let Some(uid) = node.attribute("UID").filter(|u| !u.is_empty()) else {
            return Err("AddInItem has no UID".into());
        };
        // A UID names a child of AddIns, never an arbitrary path.
        if uid.contains(['/', '\\', ':']) || matches!(uid, "." | "..") {
            return Err(format!("invalid add-in UID {uid:?}"));
        }
        if items
            .iter()
            .any(|i: &AddIn| i.uid.eq_ignore_ascii_case(uid))
        {
            return Err(format!("duplicate add-in UID {uid:?}"));
        }
        let metadata = metadata(node);
        items.push(AddIn {
            uid: uid.to_string(),
            priority: node
                .attribute("Priority")
                .and_then(|p| p.parse().ok())
                .unwrap_or(0),
            // A mods.txt entry is an explicit enable operation. Type and GameVersion
            // still gate mounting, as when the original enables an installed package.
            enabled: matches!(metadata.kind, Some(1 | 2)),
            extends: ["ExtendedModuleUID", "ExtendedModuleName", "ModuleName"]
                .into_iter()
                .filter_map(|name| node.attribute(name).filter(|s| !s.is_empty()))
                .next()
                .map(str::to_string),
            dir: std::path::PathBuf::new(),
            rank: None,
            metadata,
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_records_decode_text_and_keep_each_items_settings() {
        let xml = r#"<Manifest><AddInsList>
          <!-- <AddInItem UID="not_a_record"/> -->
          <AddInItem UID='one' Priority='12' Enabled='0' ExtendedModuleName='Single Player'>
            <Title DefaultText='fallback'><en-us><![CDATA[A & B]]></en-us></Title>
            <Description DefaultText='Some &lt;details&gt;'/><Version>9.7</Version>
            <GameVersion>1.05</GameVersion><Publisher>Someone</Publisher><Type>2</Type>
            <PrereqList><PrereqItem ProductID='7' microContentID='other' Title='Other'/>
              <PrereqItem microContentID='incomplete'/></PrereqList>
          </AddInItem>
          <AddInItem UID='two'><Type>3</Type><GameVersion>1.06</GameVersion></AddInItem>
        </AddInsList></Manifest>"#;
        let items = items(xml).unwrap();
        assert_eq!(items.len(), 2);
        let a = &items[0];
        assert!(a.enabled && a.extends_base_campaign());
        assert_eq!(a.priority, 12);
        assert_eq!(a.metadata.title, "A & B");
        assert_eq!(a.metadata.description, "Some <details>");
        assert_eq!(a.metadata.publisher, "Someone");
        assert_eq!(a.metadata.version, "9.7");
        assert_eq!(a.metadata.game_version, "1.05");
        assert_eq!(a.metadata.prerequisites.len(), 1);
        assert_eq!(a.metadata.prerequisites[0].uid, "other");
        assert_eq!(a.metadata.prerequisites[0].product_id, 7);
        assert!(!items[1].enabled);
        assert_eq!(items[1].metadata.game_version, "1.06");
        let one = xml.replace("UID='one'", "UID='one' Format='1'");
        assert!(
            super::items(&one).unwrap()[0]
                .metadata
                .prerequisites
                .is_empty()
        );
        let one = one.replace("ProductID='7'", "ProductID='7' strref='42'");
        assert_eq!(
            super::items(&one).unwrap()[0].metadata.prerequisites[0].strref,
            Some(42)
        );
    }

    #[test]
    fn utf16_packages_and_invalid_manifests() {
        let xml = "<Manifest><AddInItem UID='some_mod'><Type>1</Type></AddInItem></Manifest>";
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            bytes.extend(xml.encode_utf16().flat_map(|u| {
                if little {
                    u.to_le_bytes()
                } else {
                    u.to_be_bytes()
                }
            }));
            assert_eq!(items(&decode(&bytes).unwrap()).unwrap()[0].uid, "some_mod");
        }
        assert!(decode(&[0xff, 0xfe, 1]).is_err());
        assert!(decode(&[0xff]).is_err());
        for bad in [
            "<Manifest>",
            "<AddInItem/>",
            "<AddInItem UID='../outside'/>",
            "<Manifest><AddInItem UID='same'/><AddInItem UID='SAME'/></Manifest>",
        ] {
            assert!(items(bad).is_err(), "{bad}");
        }
    }
}
