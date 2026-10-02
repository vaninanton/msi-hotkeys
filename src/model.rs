//! The hardware gate.
//!
//! The byte map in [`crate::ec::map`] was decoded from one machine's ACPI
//! tables. On another MSI model the same windows reach different EC addresses,
//! and this program writes to one of them — so an unrecognised machine is
//! refused rather than experimented on. `--force` is there for someone who has
//! checked their own tables; it is not a flag to pass hopefully.
//!
//! The gate is the baseboard product code, not the marketing name: the ACPI
//! tables belong to the board, and MSI sells one board under several names.

use anyhow::{Context, Result};
use serde::Deserialize;
use wmi::WMIConnection;

/// Boards this map has been verified against. One so far, which is the honest
/// number.
pub const SUPPORTED: [&str; 1] = ["MS-1796"];

#[derive(Deserialize, Debug)]
#[serde(rename = "Win32_BaseBoard")]
#[serde(rename_all = "PascalCase")]
struct BaseBoard {
    product: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename = "Win32_ComputerSystemProduct")]
#[serde(rename_all = "PascalCase")]
struct SystemProduct {
    name: Option<String>,
}

/// What the machine calls itself. The serial number sits in the same classes and
/// is deliberately not read.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    /// The baseboard product code, e.g. `MS-1796`.
    pub board: Option<String>,
    /// The marketing name, e.g. `GL72 6QD`.
    pub name: Option<String>,
}

impl Identity {
    pub fn read() -> Result<Self> {
        let cimv2 = WMIConnection::with_namespace_path(r"root\cimv2")
            .context(r"cannot reach root\cimv2 to identify the machine")?;

        let boards: Vec<BaseBoard> = cimv2
            .raw_query("SELECT Product FROM Win32_BaseBoard")
            .unwrap_or_default();
        let products: Vec<SystemProduct> = cimv2
            .raw_query("SELECT Name FROM Win32_ComputerSystemProduct")
            .unwrap_or_default();

        Ok(Self {
            board: boards.into_iter().find_map(|board| board.product),
            name: products.into_iter().find_map(|product| product.name),
        })
    }

    pub fn is_supported(&self) -> bool {
        self.board
            .as_deref()
            .is_some_and(|board| SUPPORTED.contains(&board.trim()))
    }

    /// For the console and the refusal message.
    pub fn describe(&self) -> String {
        match (self.name.as_deref(), self.board.as_deref()) {
            (Some(name), Some(board)) => format!("{name} ({board})"),
            (None, Some(board)) => board.to_string(),
            (Some(name), None) => name.to_string(),
            (None, None) => "unidentified".to_string(),
        }
    }
}

/// The message shown when the gate refuses. Separate from the check so the
/// wording lives next to the reason for it.
pub fn refusal(identity: &Identity) -> String {
    format!(
        "Это не та машина: {}. Карта байтов EC снята с {}, и на другой модели те же \
         окна ведут к другим адресам — а программа в один из них пишет. \
         Запуск с --force, если вы сверили свой DSDT сами.",
        identity.describe(),
        SUPPORTED.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(board: Option<&str>, name: Option<&str>) -> Identity {
        Identity {
            board: board.map(str::to_string),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn accepts_the_board_it_was_mapped_from() {
        assert!(identity(Some("MS-1796"), Some("GL72 6QD")).is_supported());
        // Firmware pads these strings often enough to be worth handling.
        assert!(identity(Some("MS-1796  "), None).is_supported());
    }

    #[test]
    fn refuses_anything_else() {
        assert!(!identity(Some("MS-16Q2"), Some("GS65 Stealth")).is_supported());
        assert!(!identity(None, Some("GL72 6QD")).is_supported());
        assert!(!Identity::default().is_supported());
    }

    #[test]
    fn describes_what_it_found() {
        assert_eq!(
            identity(Some("MS-1796"), Some("GL72 6QD")).describe(),
            "GL72 6QD (MS-1796)"
        );
        assert_eq!(identity(Some("MS-16Q2"), None).describe(), "MS-16Q2");
        assert_eq!(Identity::default().describe(), "unidentified");
    }

    #[test]
    fn the_refusal_names_the_machine_and_the_way_out() {
        let text = refusal(&identity(Some("MS-16Q2"), Some("GS65 Stealth")));
        assert!(text.contains("GS65 Stealth (MS-16Q2)"));
        assert!(text.contains("MS-1796"));
        assert!(text.contains("--force"));
    }
}
