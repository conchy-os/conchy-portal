//! `xdg-desktop-portal-conchy` — headless-маршрутизатор вызовов `xdg-desktop-portal`.
//! Без GPUI и без UI: отвечает сам (Settings) или передаёт задачу хосту UI.

mod request;
mod session;
mod settings;
mod sources;
#[cfg(test)]
mod testutil;

use anyhow::Context;
use conchy_portal_proto::{DAEMON_BUS_NAME, PORTAL_OBJECT_PATH};
use tokio::signal::unix::{SignalKind, signal};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    // Интерфейсы регистрируем до RequestName: вызовы, пришедшие сразу после захвата имени,
    // не должны потеряться.
    let store = settings::Store::default();
    let dir = sources::config_dir();
    store.replace(sources::load_all(&dir, &settings::Map::new()));

    let conn = zbus::connection::Builder::session()
        .context("нет сессионной шины")?
        .serve_at(PORTAL_OBJECT_PATH, settings::Settings::new(store.clone()))
        .context("не удалось зарегистрировать Settings")?
        .name(DAEMON_BUS_NAME)
        .context("некорректное имя шины")?
        .build()
        .await
        .context("не удалось занять имя на шине (демон уже запущен?)")?;
    let iface = conn
        .object_server()
        .interface::<_, settings::Settings>(PORTAL_OBJECT_PATH)
        .await?;
    let _watcher = sources::spawn_watcher(dir, store, iface).context("не запустить наблюдатель конфигов")?;
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
