//! Deriving the single working image for analysis. Delegates to
//! [`GelDocument::working_image`], which HDR-merges a bracket when present.

use opengel::core::{GelDocument, GrayF32};

pub fn working_image(doc: &GelDocument) -> Option<GrayF32> {
    doc.working_image()
}
