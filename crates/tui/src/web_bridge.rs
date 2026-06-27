//! Glue between the TUI's concrete `App::live: Arc<Live>` and the web crate's
//! `Arc<dyn LiveRead + Send + Sync>`. Lives in the TUI (not the web crate)
//! because the web crate must not depend on `cyberdeck-tui`.
//!
//! The TUI's main loop owns one tap task that consumes WebStart/WebStop
//! actions from a dedicated control channel; that task then calls
//! `cyberdeck_web::run_with` with a `TuiLiveRead` adapter (defined here) and
//! a toast pump that converts web `Action`s back into TUI `Action::Toast`s.
//!
//! Why a separate struct and not a blanket impl on `App::live`? Because the
//! `LiveRead` trait is in the web crate; we don't want to add a web dep to
//! `cyberdeck-core`, and we don't want the TUI's `App` to have a circular dep
//! on `cyberdeck-web`. `TuiLiveRead` is the seam.

#[cfg(feature = "web")]
mod inner {
    use std::sync::Arc;

    use cyberdeck_core::net::Interface;
    use cyberdeck_core::power::Battery;
    use cyberdeck_core::sys::{SystemInfo, ThermalReading};
    use cyberdeck_core::{audio, bluetooth, display, net, packages, process, services, storage};
    use tokio::sync::mpsc::Sender;

    use cyberdeck_web::api::LiveRead;
    use cyberdeck_web::run::toast_compat::Action as WebAction;

    use crate::app::Live;

    /// The bridge. It implements the web's `LiveRead` against the TUI's `Live`.
    /// `action_tx` is the sender half of the TUI's action channel, wrapped to
    /// turn `WebAction` into TUI `Action::Toast` so the web UI can notify the
    /// user of side effects.
    pub struct TuiLiveRead {
        pub live: Arc<Live>,
        pub action_tx: Sender<super::AppAction>,
    }

    #[async_trait::async_trait]
    impl LiveRead for TuiLiveRead {
        async fn info(&self) -> SystemInfo {
            self.live.info.read().await.clone()
        }
        async fn battery(&self) -> Option<Battery> {
            self.live.battery.read().await.clone()
        }
        async fn thermals(&self) -> Vec<ThermalReading> {
            self.live.thermals.read().await.clone()
        }
        async fn interfaces(&self) -> Vec<Interface> {
            self.live.interfaces.read().await.clone()
        }
        async fn active_ssid(&self) -> Option<String> {
            self.live.active_ssid.read().await.clone()
        }
        async fn services(&self) -> Vec<services::Service> {
            self.live.services.read().await.clone()
        }
        async fn filesystems(&self) -> Vec<storage::Filesystem> {
            self.live.filesystems.read().await.clone()
        }
        async fn upgradable(&self) -> Vec<packages::Package> {
            self.live.upgradable.read().await.clone()
        }
        async fn processes(&self) -> Vec<process::Process> {
            self.live.processes.read().await.clone()
        }
        async fn displays(&self) -> Vec<display::DisplayOutput> {
            self.live.displays.read().await.clone()
        }
        async fn sinks(&self) -> Vec<audio::Sink> {
            self.live.sinks.read().await.clone()
        }
        async fn bluetooth(&self) -> Vec<bluetooth::BtDevice> {
            self.live.bluetooth.read().await.clone()
        }
    }

    /// Convert a `WebAction` (an action sent by the web UI) into an
    /// `AppAction` (a TUI action). The web only emits `Toast`, so this is
    /// the only conversion we need.
    pub fn web_to_app(a: WebAction) -> super::AppAction {
        let kind = match a.kind {
            "ok" => crate::app::toast::ToastKind::Ok,
            "warn" => crate::app::toast::ToastKind::Warn,
            _ => crate::app::toast::ToastKind::Error,
        };
        super::AppAction::Toast(kind, a.message)
    }
}

#[cfg(feature = "web")]
pub use inner::{web_to_app, TuiLiveRead};

/// A type alias for the TUI's action enum, so this file doesn't have to import
/// the whole `Action` machinery just to spell the type.
pub type AppAction = crate::app::action::Action;
