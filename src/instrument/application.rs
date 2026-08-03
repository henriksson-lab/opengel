//! Applications — sample type × detection reagent.
//!
//! An application is the experiment-level thing a user actually picks ("protein
//! gel, Coomassie Blue"). It *implies a tray*, because the reagent determines
//! which light source excites it, and the tray is the light source. That makes
//! the application the natural place to catch the most common setup mistake:
//! the right dye on the wrong tray, which images as a blank gel.
//!
//! The reagent lists come from the tray catalogue in the instrument guide. A
//! reagent that works under two light sources (SYBR Green under both UV and
//! blue) appears once per tray, because they are genuinely different
//! acquisitions.

use crate::core::model::GelType;

use super::TrayType;

/// What is being imaged, independent of the dye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleKind {
    NucleicAcid,
    Protein,
}

impl SampleKind {
    pub fn label(self) -> &'static str {
        match self {
            SampleKind::NucleicAcid => "DNA / RNA gel",
            SampleKind::Protein => "Protein gel",
        }
    }

    /// The document gel type to start from. Nucleic-acid runs open as DNA
    /// because base pairs are the common case; the user can switch a document to
    /// RNA afterwards without affecting the acquisition.
    pub fn gel_type(self) -> GelType {
        match self {
            SampleKind::NucleicAcid => GelType::Dna,
            SampleKind::Protein => GelType::Protein,
        }
    }
}

/// One selectable application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Application {
    /// Stable identifier, safe to persist in a protocol.
    pub id: &'static str,
    /// The detection reagent, as a user would name it.
    pub reagent: &'static str,
    pub sample: SampleKind,
    /// The tray this reagent needs.
    pub tray: TrayType,
    /// Whether this application needs the stain-free UV activation step before
    /// the exposure.
    pub needs_activation: bool,
}

impl Application {
    /// "Protein gel — Coomassie Blue".
    pub fn label(&self) -> String {
        format!("{} — {}", self.sample.label(), self.reagent)
    }

    pub fn gel_type(&self) -> GelType {
        self.sample.gel_type()
    }
}

const fn app(
    id: &'static str,
    reagent: &'static str,
    sample: SampleKind,
    tray: TrayType,
) -> Application {
    Application {
        id,
        reagent,
        sample,
        tray,
        needs_activation: false,
    }
}

const fn stain_free(id: &'static str, reagent: &'static str) -> Application {
    Application {
        id,
        reagent,
        sample: SampleKind::Protein,
        tray: TrayType::StainFree,
        needs_activation: true,
    }
}

/// Every predefined application, grouped by the tray it implies.
pub const APPLICATIONS: &[Application] = &[
    // --- UV transilluminator ---
    app("uv-etbr", "Ethidium bromide", SampleKind::NucleicAcid, TrayType::Uv),
    app("uv-sybr-green", "SYBR Green", SampleKind::NucleicAcid, TrayType::Uv),
    app("uv-sybr-safe", "SYBR Safe", SampleKind::NucleicAcid, TrayType::Uv),
    app("uv-sybr-gold", "SYBR Gold", SampleKind::NucleicAcid, TrayType::Uv),
    app("uv-gelgreen", "GelGreen", SampleKind::NucleicAcid, TrayType::Uv),
    app("uv-gelred", "GelRed", SampleKind::NucleicAcid, TrayType::Uv),
    app("uv-flamingo", "Flamingo", SampleKind::Protein, TrayType::Uv),
    app("uv-oriole", "Oriole", SampleKind::Protein, TrayType::Uv),
    app(
        "uv-coomassie-fluor-orange",
        "Coomassie Fluor Orange",
        SampleKind::Protein,
        TrayType::Uv,
    ),
    app("uv-sypro-ruby", "SYPRO Ruby", SampleKind::Protein, TrayType::Uv),
    app("uv-krypton", "Krypton", SampleKind::Protein, TrayType::Uv),
    // --- White light ---
    app(
        "white-fast-blast",
        "Fast Blast DNA stain",
        SampleKind::NucleicAcid,
        TrayType::White,
    ),
    app("white-coomassie-blue", "Coomassie Blue", SampleKind::Protein, TrayType::White),
    app("white-copper", "Copper stain", SampleKind::Protein, TrayType::White),
    app("white-zinc", "Zinc stain", SampleKind::Protein, TrayType::White),
    app("white-silver", "Silver stain", SampleKind::Protein, TrayType::White),
    // --- Blue light ---
    app("blue-sybr-green", "SYBR Green", SampleKind::NucleicAcid, TrayType::Blue),
    app("blue-sybr-safe", "SYBR Safe", SampleKind::NucleicAcid, TrayType::Blue),
    app("blue-sybr-gold", "SYBR Gold", SampleKind::NucleicAcid, TrayType::Blue),
    app("blue-gelgreen", "GelGreen", SampleKind::NucleicAcid, TrayType::Blue),
    // --- Stain-free ---
    stain_free("stainfree-gel", "Stain-free gel"),
    stain_free("stainfree-blot", "Stain-free blot"),
];

/// Look an application up by its persisted id.
pub fn by_id(id: &str) -> Option<&'static Application> {
    APPLICATIONS.iter().find(|a| a.id == id)
}

/// Applications a given tray supports, in catalogue order.
pub fn for_tray(tray: TrayType) -> Vec<&'static Application> {
    APPLICATIONS.iter().filter(|a| a.tray == tray).collect()
}

/// A sensible application to preselect when `tray` is inserted.
pub fn default_for_tray(tray: TrayType) -> &'static Application {
    for_tray(tray)
        .first()
        .copied()
        // Every tray has at least one application, but fall back rather than
        // panic in a UI path.
        .unwrap_or(&APPLICATIONS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = APPLICATIONS.iter().map(|a| a.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "application ids must be unique to persist");
    }

    #[test]
    fn every_tray_has_applications() {
        for tray in TrayType::ALL {
            assert!(!for_tray(tray).is_empty(), "{tray:?} has no applications");
        }
    }

    #[test]
    fn a_reagent_usable_on_two_trays_appears_once_per_tray() {
        // SYBR Green works under UV and under blue light; they are different
        // acquisitions, and picking one must pin the tray.
        let sybr: Vec<_> = APPLICATIONS.iter().filter(|a| a.reagent == "SYBR Green").collect();
        assert_eq!(sybr.len(), 2);
        assert_ne!(sybr[0].tray, sybr[1].tray);
    }

    #[test]
    fn only_stain_free_applications_activate() {
        for a in APPLICATIONS {
            assert_eq!(
                a.needs_activation,
                a.tray == TrayType::StainFree,
                "{}: activation is a stain-free-only step",
                a.id
            );
        }
    }

    #[test]
    fn ids_round_trip_through_lookup() {
        for a in APPLICATIONS {
            assert_eq!(by_id(a.id).map(|f| f.id), Some(a.id));
        }
        assert!(by_id("no-such-application").is_none());
    }
}
