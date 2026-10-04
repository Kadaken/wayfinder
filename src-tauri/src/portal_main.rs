//! Stage-one entry point for the `org.freedesktop.impl.portal.desktop.wayfinder`
//! backend. Owns the bus name and exports only
//! `org.freedesktop.impl.portal.FileChooser.OpenFile`. Must never import
//! or call into `native.rs`, and must never initialize GTK, GStreamer or
//! a display -- see `portal_service.rs` for all service logic.

mod portal_service;

use portal_service::{PortalService, UnavailableChooser, BUS_NAME, OBJECT_PATH};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let chooser = Arc::new(UnavailableChooser);
    let service = PortalService::new(chooser);

    let _connection = zbus::blocking::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, service)?
        .build()?;

    loop {
        std::thread::park();
    }
}
