//! `xdg-desktop-portal-conchy` — headless-маршрутизатор вызовов `xdg-desktop-portal`.
//! Без GPUI и без UI: отвечает сам (Settings) или передаёт задачу хосту UI.

mod access;
mod app_chooser;
mod file_chooser;
mod hosts;
mod inhibit;
mod mutter;
mod notification;
mod opts;
mod request;
mod screencast;
mod screenshot;
mod session;
mod settings;
mod sources;
#[cfg(test)]
mod testutil;
mod uri;

use anyhow::Context;
use conchy_portal_proto::{DAEMON_BUS_NAME, PORTAL_OBJECT_PATH};
use tokio::signal::unix::{SignalKind, signal};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Интерфейсы регистрируем до RequestName: вызовы, пришедшие сразу после захвата имени,
    // не должны потеряться.
    let store = settings::Store::default();
    let dir = sources::config_dir();
    store.replace(sources::load_all(&dir, &settings::Map::new()));

    let notification_iface = notification::Notification::default();
    let conn = zbus::connection::Builder::session()
        .context("нет сессионной шины")?
        .serve_at(PORTAL_OBJECT_PATH, settings::Settings::new(store.clone()))
        .context("не удалось зарегистрировать Settings")?
        .serve_at(PORTAL_OBJECT_PATH, screenshot::Screenshot::from_env())
        .context("не удалось зарегистрировать Screenshot")?
        .serve_at(PORTAL_OBJECT_PATH, file_chooser::FileChooser::new())
        .context("не удалось зарегистрировать FileChooser")?
        .serve_at(
            PORTAL_OBJECT_PATH,
            app_chooser::AppChooser { activate: true },
        )
        .context("не удалось зарегистрировать AppChooser")?
        .serve_at(PORTAL_OBJECT_PATH, access::Access { activate: true })
        .context("не удалось зарегистрировать Access")?
        .serve_at(PORTAL_OBJECT_PATH, screencast::ScreenCast::new(true))
        .context("не удалось зарегистрировать ScreenCast")?
        .serve_at(PORTAL_OBJECT_PATH, inhibit::Inhibit)
        .context("не удалось зарегистрировать Inhibit")?
        .serve_at(PORTAL_OBJECT_PATH, notification_iface.clone())
        .context("не удалось зарегистрировать Notification")?
        .name(DAEMON_BUS_NAME)
        .context("некорректное имя шины")?
        .build()
        .await
        .context("не удалось занять имя на шине (демон уже запущен?)")?;
    let iface = conn
        .object_server()
        .interface::<_, settings::Settings>(PORTAL_OBJECT_PATH)
        .await?;
    {
        let iface = conn
            .object_server()
            .interface::<_, notification::Notification>(PORTAL_OBJECT_PATH)
            .await?;
        let conn = conn.clone();
        let n = notification_iface.clone();
        tokio::spawn(async move {
            if let Err(e) = n.forward_signals(conn, iface).await {
                tracing::warn!("ActionInvoked не пробрасывается: {e}");
            }
        });
    }
    let _watcher =
        sources::spawn_watcher(dir, store, iface).context("не запустить наблюдатель конфигов")?;
    info!(
        name = DAEMON_BUS_NAME,
        startup_ms = started.elapsed().as_secs_f64() * 1000.0,
        "демон портала запущен"
    );

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = term.recv() => info!("SIGTERM"),
        _ = int.recv() => info!("SIGINT"),
    }
    drop(conn);
    Ok(())
}
