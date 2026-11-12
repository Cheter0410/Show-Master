//! Tray icon (macOS menu bar, Windows notification area, Linux
//! StatusNotifierItem): shows that Show Master keeps running while its window
//! is closed. Menu: "Open Show Master", the address for tablets, "Quit Show
//! Master". A click on the icon opens the window (Windows, Linux).
//!
//! Optional: when the desktop has no tray (e.g. GNOME without the
//! AppIndicator extension) nothing is shown and closing the window minimizes
//! it instead of hiding it (see [`crate::background::can_hide`]).

use crate::background;

const OPEN: &str = "Open Show Master";
const QUIT: &str = "Quit Show Master";
const TOOLTIP: &str = "Show Master - keeps running while the window is closed";

/// RGBA pixels of the app icon at `size` x `size`.
fn icon_rgba(size: u32) -> Option<Vec<u8>> {
    let img = image::load_from_memory(include_bytes!("../../../assets/icon.png")).ok()?;
    Some(
        img.resize_exact(size, size, image::imageops::FilterType::Triangle)
            .to_rgba8()
            .into_raw(),
    )
}

fn address_label(address: Option<&str>) -> String {
    match address {
        Some(a) => format!("Tablets: {a}"),
        None => "Tablets: remote off".to_string(),
    }
}

/// The running tray icon; dropping it removes the icon.
pub struct Tray {
    address: Option<String>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    _icon: tray_icon::TrayIcon,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    info: tray_icon::menu::MenuItem,
    #[cfg(target_os = "linux")]
    handle: ksni::Handle<linux::SmTray>,
    #[cfg(target_os = "linux")]
    rt: tokio::runtime::Handle,
}

impl Tray {
    /// Update the "Tablets: ..." line (the Bonjour name is known a moment
    /// after start, the network can change).
    pub fn set_address(&mut self, address: Option<String>) {
        if self.address == address {
            return;
        }
        self.address = address.clone();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.info.set_text(address_label(address.as_deref()));
        #[cfg(target_os = "linux")]
        {
            let handle = self.handle.clone();
            self.rt.spawn(async move {
                handle.update(|t| t.address = address).await;
            });
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            drop(self.handle.shutdown());
        }
        background::set_tray_available(false);
    }
}

/// Start the tray icon. `address` is shown in the menu (e.g.
/// `showmaster.local`). Must be called on the main thread once the event loop
/// runs (macOS, Windows).
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn start(address: Option<String>, _rt: &tokio::runtime::Handle) -> Option<Tray> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let menu = Menu::new();
    let open = MenuItem::with_id("open", OPEN, true, None);
    let quit = MenuItem::with_id("quit", QUIT, true, None);
    let info = MenuItem::with_id("address", address_label(address.as_deref()), false, None);
    let mut ok = menu.append(&open).is_ok();
    ok &= menu.append(&info).is_ok();
    ok &= menu.append(&PredefinedMenuItem::separator()).is_ok();
    ok &= menu.append(&quit).is_ok();
    if !ok {
        tracing::warn!("tray menu could not be built");
    }
    MenuEvent::set_event_handler(Some(|e: MenuEvent| match e.id.0.as_str() {
        "open" => background::request_show(),
        "quit" => background::request_quit(),
        _ => {}
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            background::request_show();
        }
    }));
    let size = if cfg!(target_os = "macos") { 44 } else { 32 };
    let mut builder = TrayIconBuilder::new()
        .with_id("showmaster")
        .with_tooltip(TOOLTIP)
        .with_menu(Box::new(menu))
        // macOS: a click opens the menu (convention); Windows: a left click
        // opens the window, the right click the menu.
        .with_menu_on_left_click(cfg!(target_os = "macos"));
    if let Some(icon) =
        icon_rgba(size).and_then(|rgba| tray_icon::Icon::from_rgba(rgba, size, size).ok())
    {
        builder = builder.with_icon(icon);
    }
    match builder.build() {
        Ok(icon) => {
            background::set_tray_available(true);
            Some(Tray {
                address,
                _icon: icon,
                info,
            })
        }
        Err(e) => {
            tracing::warn!("no tray icon: {e}");
            None
        }
    }
}

#[cfg(target_os = "linux")]
pub fn start(address: Option<String>, rt: &tokio::runtime::Handle) -> Option<Tray> {
    use ksni::TrayMethods;
    let tray = linux::SmTray {
        address: address.clone(),
        icon: icon_rgba(32).map(linux::argb).unwrap_or_default(),
    };
    // Registering with the desktop's tray host is a few D-Bus round trips;
    // wait briefly so closing the window knows whether the tray exists.
    let spawned = rt.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(2), tray.spawn()).await
    });
    match spawned {
        Ok(Ok(handle)) => {
            background::set_tray_available(true);
            Some(Tray {
                address,
                handle,
                rt: rt.clone(),
            })
        }
        Ok(Err(e)) => {
            tracing::info!("no tray icon on this desktop ({e}); closing the window minimizes it");
            None
        }
        Err(_) => {
            tracing::info!(
                "no tray icon (desktop did not answer); closing the window minimizes it"
            );
            None
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn start(_address: Option<String>, _rt: &tokio::runtime::Handle) -> Option<Tray> {
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{address_label, background, OPEN, QUIT, TOOLTIP};

    pub struct SmTray {
        pub address: Option<String>,
        /// 32x32 ARGB32, network byte order.
        pub icon: Vec<u8>,
    }

    /// RGBA -> ARGB (StatusNotifierItem pixmap format).
    pub fn argb(rgba: Vec<u8>) -> Vec<u8> {
        rgba.chunks_exact(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect()
    }

    impl ksni::Tray for SmTray {
        fn id(&self) -> String {
            "showmaster".into()
        }

        fn title(&self) -> String {
            "Show Master".into()
        }

        fn icon_name(&self) -> String {
            // Installed by the packages; the pixmap below is the fallback.
            "showmaster".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            if self.icon.is_empty() {
                return Vec::new();
            }
            vec![ksni::Icon {
                width: 32,
                height: 32,
                data: self.icon.clone(),
            }]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: TOOLTIP.into(),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            background::request_show();
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            let mut items: Vec<ksni::MenuItem<Self>> = vec![StandardItem {
                label: OPEN.into(),
                activate: Box::new(|_: &mut Self| background::request_show()),
                ..Default::default()
            }
            .into()];
            items.push(
                StandardItem {
                    label: address_label(self.address.as_deref()),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
            items.push(ksni::MenuItem::Separator);
            items.push(
                StandardItem {
                    label: QUIT.into(),
                    activate: Box::new(|_: &mut Self| background::request_quit()),
                    ..Default::default()
                }
                .into(),
            );
            items
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn rgba_to_argb() {
            assert_eq!(
                super::argb(vec![1, 2, 3, 4, 5, 6, 7, 8]),
                vec![4, 1, 2, 3, 8, 5, 6, 7]
            );
        }
    }
}
