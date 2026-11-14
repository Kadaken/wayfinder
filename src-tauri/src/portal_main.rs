// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Kadaken

//! Stage-one entry point for the `org.freedesktop.impl.portal.desktop.wayfinder`
//! backend. Owns the bus name and exports only
//! `org.freedesktop.impl.portal.FileChooser.OpenFile`. Must never import
//! or call into `native.rs`, and must never initialize GTK, GStreamer or
//! a display -- see `portal_service.rs` for all service logic.

mod crash;
mod portal_service;

use portal_service::{PortalService, ProcessChooser, BUS_NAME, OBJECT_PATH};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    crash::install("wayfinder-portal");
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("wayfinder-portal {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let chooser = Arc::new(ProcessChooser::production());
    let service = PortalService::new(chooser);

    // D-249: ask for the name, and refuse to queue for it. Without
    // `DoNotQueue` a second copy does not fail -- it joins the bus queue and
    // then parks here for ever, because nothing below ever exits. Thirteen
    // copies were found alive on 7 November 2026, twelve of them queued and
    // idle for 26 hours; only the first owned the name and answered anything.
    let connection = zbus::blocking::connection::Builder::session()?
        .serve_at(OBJECT_PATH, service)?
        .build()?;
    let owned = connection.request_name_with_flags(
        BUS_NAME,
        zbus::fdo::RequestNameFlags::DoNotQueue.into(),
    );
    // `DoNotQueue` turns "somebody else has it" into an error rather than a
    // reply, so both shapes mean the same thing here: this copy is not the
    // backend and must go. It leaves with 0, because a non-zero exit makes
    // D-Bus activation report a failure for a situation that is not one.
    let is_owner = matches!(owned, Ok(zbus::fdo::RequestNameReply::PrimaryOwner));
    if !is_owner {
        eprintln!(
            "wayfinder-portal: {BUS_NAME} is already served by another copy, \
             so this one is not needed and is exiting."
        );
        return Ok(());
    }

    loop {
        std::thread::park();
    }
}
