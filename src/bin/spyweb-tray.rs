#![windows_subsystem = "windows"]

use anyhow::Result;

#[cfg(feature = "tray")]
struct App {
    open_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

#[cfg(feature = "tray")]
impl winit::application::ApplicationHandler for App {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        use tray_icon::menu::MenuEvent;
        use winit::event_loop::ControlFlow;

        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.quit_id {
                spyweb::cdp::browser::shutdown_all();
                spyweb::services::io::shutdown();
                std::thread::sleep(std::time::Duration::from_millis(500));
                event_loop.exit();
            } else if event.id == self.open_id {                let url = format!("http://{}", spyweb::config::get_base_url());
                let _ = open::that(&url);
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(100),
        ));
    }
}

#[cfg(feature = "tray")]
fn main() -> Result<()> {
    use spyweb::entry;
    use std::thread;
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder};
    use winit::event_loop::EventLoop;

    thread::spawn(|| {
        if let Err(e) = entry::start_app() {
            spyweb::t_eprintln!("SpyWeb Background Engine Error: {}", e);
        }
    });

    let tray_menu = Menu::new();
    let open_i = MenuItem::new("Open Web UI", true, None);
    let quit_i = MenuItem::new("Quit SpyWeb", true, None);
    tray_menu.append_items(&[&open_i, &PredefinedMenuItem::separator(), &quit_i])?;
    let icon = Icon::from_rgba(include_bytes!("../../icon-32x32.rgba").to_vec(), 32, 32)?;

    let _tray = TrayIconBuilder::new()
        .with_tooltip("SpyWeb")
        .with_icon(icon)
        .with_menu(Box::new(tray_menu))
        .build()?;

    let event_loop = EventLoop::builder().build()?;

    let mut app = App {
        open_id: open_i.id().clone(),
        quit_id: quit_i.id().clone(),
    };

    event_loop.run_app(&mut app)?;

    Ok(())
}

#[cfg(not(feature = "tray"))]
fn main() -> Result<()> {
    eprintln!(
        "{}",
        spyweb::color::c_err(
            "Error: This binary must be compiled with the 'tray' feature enabled."
        )
    );
    std::process::exit(1);
}
