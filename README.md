# conchy-portal

Свой бэкенд `xdg-desktop-portal` для Conchy OS: `org.freedesktop.impl.portal.desktop.conchy`.
Rust, без GTK/Qt. Демон — маленький headless-маршрутизатор; диалоги показывают уже запущенные
программы Conchy (`conchy-files`, `conchy-shell`). План и журнал решений — [PLAN.md](PLAN.md).

| Интерфейс | Где живёт UI | Состояние |
|---|---|---|
| Settings (тема, акцент, контраст, курсор…) | демон | готово, `conchy.portal` включён |
| Screenshot, PickColor | `conchy-wm`, `conchy-shot --portal-output` | готово |
| FileChooser | `conchy-files` (`org.conchy.Portal.FilesHost`) | готово |
| AppChooser | `conchy-files` | готово |
| Access | `conchy-shell` (`org.conchy.Portal.ShellHost`) | готово |
| ScreenCast | пикер в `conchy-shell`, потоки — эмуляция Mutter в `conchy-wm` | реализовано, **в конфиге выключено** до проверки на реальных приложениях |
| Inhibit, Notification | демон → `org.freedesktop.ScreenSaver` / `org.freedesktop.Notifications` | реализовано (тесты с моками), в конфиге не включено |

```
crates/proto    # типы и константы протокола демон ↔ хосты (без zbus/tokio/gpui)
crates/daemon   # бинарь xdg-desktop-portal-conchy
data/           # .portal, D-Bus .service, conchy-portals.conf
scripts/        # dev-install.sh, private-bus.sh, e2e-env.sh, e2e/*.py
packaging/void  # шаблон xbps-src (не проверен сборкой)
```

## Разработка

```sh
cargo test                      # тесты демона на p2p-соединениях zbus (без xdp и композитора)
cargo clippy --all-targets
scripts/dev-install.sh          # поставить .portal/.service/conf в ~/.local и ~/.config
```

Изолированная проверка с настоящим `xdg-desktop-portal` на приватной шине:
`source scripts/e2e-env.sh debug` (поднимает шину, xdp и демон с `XDG_CURRENT_DESKTOP=conchy`).
Скрипты в `scripts/e2e/` — это интеграционные проверки; **те из них, что шлют клавиши через
`hyprctl`, нажимают их в живой сессии**, запускайте осознанно.

Зависит по path от соседних репозиториев (`../conchy-config` — парсер темы). Хосты UI зависят
от `crates/proto` по path из `conchy-files` и `conchy-shell`.
