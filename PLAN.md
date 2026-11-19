# conchy-portal: план собственного бэкенда xdg-desktop-portal

> Документ для исполнителя (отдельный чат, более дешёвая модель). Читай целиком перед началом.
> Фазы выполняются строго по порядку; каждая заканчивается проверяемым «Готово, если».
> Если что-то в плане противоречит коду или спецификации — **прав код и спека**; зафиксируй расхождение
> в разделе «Журнал решений» в конце этого файла и продолжай.

---

## 0. Цель и принципы

**Цель:** заменить `xdg-desktop-portal-gnome` (GTK4 + libadwaita), `xdg-desktop-portal-gtk` (GTK3) и
`xdg-desktop-portal-hyprland` (Qt-пикер) на свой бэкенд
`org.freedesktop.impl.portal.desktop.conchy`, написанный на Rust, с UI на GPUI в уже существующих
программах Conchy.

**Принципы (не нарушать):**

1. **Rust only. Никаких GTK/Qt/C-библиотек** в новом коде. Разрешено: `zbus`, `zvariant`, `tokio`,
   `serde`, `notify`, `tracing`, `anyhow`/`thiserror`, крейты из `conchy-*`.
2. **Демон портала — без GPUI и без UI.** Это маленький резидентный процесс-маршрутизатор: принимает
   вызовы от `xdg-desktop-portal`, отвечает сам (Settings) или передаёт задачу «хосту UI».
3. **UI живёт в уже тёплых процессах**, чтобы диалог появлялся мгновенно:
   - `conchy-files` — FileChooser и AppChooser (у него уже есть сетка файлов, `openwith.rs`, `apps.rs`, `mime.rs`);
   - `conchy-shell` — Access (запрос разрешений), пикер ScreenCast (у шелла уже есть layer-shell
     overlay, тема, сервис композитора со списком окон/выходов).
4. **Скорость — требование, а не пожелание** (см. бюджеты в §2). Каждая фаза меряет свои цифры
   через `tracing` (span от входа вызова до первого кадра/ответа).
5. **Падение хоста UI не роняет демон.** Ошибка вызова хоста → ответ порталу `response = 2` (other).
6. Стиль кода — как в соседнем крейте, который правишь (в `conchy-files` комментарии на русском, в
   `conchy-shell` на английском). Edition 2024, `cargo clippy` без новых предупреждений.

---

## 1. Что уже есть (проверено по коду)

| Где | Что | Как используем |
|---|---|---|
| `conchy-session/src/env.rs:69` | `XDG_CURRENT_DESKTOP=conchy` | xdp ищет `conchy-portals.conf` |
| `conchy-session/src/dbus.rs` | запуск dbus-daemon + `UpdateActivationEnvironment` | D-Bus-активация нашего демона получит `WAYLAND_DISPLAY` |
| `conchy-wm/resources/conchy-portals.conf` | сейчас `default=gnome;gtk;` | будем постепенно переключать на `conchy` |
| `conchy-wm/src/dbus/mutter_screen_cast.rs` | эмуляция `org.gnome.Mutter.ScreenCast` + PipeWire (`src/screencasting/`) | основа ScreenCast без своего PipeWire-кода |
| `conchy-wm/src/dbus/mutter_display_config.rs` | `org.gnome.Mutter.DisplayConfig` | список мониторов для пикера |
| `conchy-wm/src/dbus/gnome_shell_introspect.rs` | `org.gnome.Shell.Introspect.GetWindows` | список окон + `window-id` для `RecordWindow` |
| `conchy-wm/src/dbus/gnome_shell_screenshot.rs` | `org.gnome.Shell.Screenshot` (снимок, `PickColor`) | Screenshot/PickColor почти бесплатно |
| `conchy-shot/` | интерактивный скриншотер на layer-shell | интерактивный режим Screenshot |
| `conchy-config/src/bus.rs` | `org.conchy.Config` (Get/Set/List, сигнал `PropertyChanged`) | живые изменения настроек |
| `conchy-shell/crates/app/src/config/theme/persistence.rs` | тема хранится в `~/.config/conchy/theme.kdl` (`StoredTheme`: `name`, `base16`, `colors`…) | источник color-scheme/accent |
| `conchy-shell/crates/ui/src/theme/` | `is_dark()`, `colors.accent` | логика «тёмная/светлая», акцент |
| `conchy-files/src/fm1.rs`, `instance.rs` | zbus-сервис `FileManager1`, single-instance через сокет | образец, как поднять D-Bus-интерфейс в conchy-files |
| `conchy-files/src/ui/openwith.rs`, `src/apps.rs`, `src/mime.rs` | список приложений по MIME, mimeapps.list, запуск | AppChooser |
| `conchy-files/src/ui/dialogs.rs` | модальные диалоги поверх окна | подтверждение перезаписи в Save |
| `conchy-shell/crates/app/src/windows.rs:550` | окна `WindowKind::LayerShell` (`Layer::Overlay`) | модальные окна Access и пикера ScreenCast |

**Не проверено, проверить в Фазе 0** (записать результат в журнал):
- версия `xdg-desktop-portal` в системе (`/usr/libexec/xdg-desktop-portal --version`); от неё зависят
  `accent-color`, `contrast`, `reduced-motion`, версии интерфейсов;
- XML-описания impl-интерфейсов: `/usr/share/dbus-1/interfaces/org.freedesktop.impl.portal.*.xml` —
  **это главный источник сигнатур**, а не этот план;
- совпадает ли id окна из `conchy-ipc` (которым пользуется шелл) с `window-id` из
  `org.gnome.Shell.Introspect` / `RecordWindow` (в niri оба — `MappedId`, но проверить);
- есть ли в шелле уже готовый каркас для собственного zbus-сервера (`crates/services/src/bus.rs`).

---

## 2. Архитектура

```
 приложение (Firefox, Flatpak, GTK, Qt…)
        │  org.freedesktop.portal.*         (фронтенд, системный пакет)
        ▼
 xdg-desktop-portal ── читает conchy-portals.conf
        │  org.freedesktop.impl.portal.*
        ▼
 xdg-desktop-portal-conchy   (НОВОЕ: headless демон, tokio + zbus)
   ├─ Settings ........ отвечает сам (theme.kdl + conchy-config, inotify)
   ├─ Screenshot ...... org.gnome.Shell.Screenshot в conchy-wm / conchy-shot
   ├─ FileChooser ─┐
   ├─ AppChooser ──┴─► org.conchy.Portal.FilesHost  (в conchy-files)
   ├─ Access ──────┐
   ├─ ScreenCast ──┴─► org.conchy.Portal.ShellHost  (в conchy-shell)  — выбор источника
   │                   + org.gnome.Mutter.ScreenCast (в conchy-wm)    — сам поток PipeWire
   └─ (позже) Notification, Inhibit, Wallpaper
```

### 2.1 Раскладка репозитория

Новый воркспейс `conchy-portal/` (рядом с остальными `conchy-*`):

```
conchy-portal/
├── Cargo.toml                    # workspace, resolver = "3", edition 2024
├── PLAN.md                       # этот файл
├── crates/
│   ├── proto/                    # conchy-portal-proto: ТОЛЬКО типы и константы (serde + zvariant)
│   │   └── src/lib.rs            #   имена шин/путей/интерфейсов хостов, структуры запросов/ответов
│   └── daemon/                   # бинарь xdg-desktop-portal-conchy
│       └── src/
│           ├── main.rs           # tokio runtime, регистрация имени, serve_at всех интерфейсов
│           ├── request.rs        # объект org.freedesktop.impl.portal.Request (Close) + guard
│           ├── session.rs        # объект org.freedesktop.impl.portal.Session (Close, Closed)
│           ├── settings.rs
│           ├── screenshot.rs
│           ├── file_chooser.rs
│           ├── app_chooser.rs
│           ├── access.rs
│           ├── screencast.rs
│           ├── hosts.rs          # #[proxy] к FilesHost/ShellHost + запуск/активация хостов
│           └── appearance.rs     # вычисление color-scheme/accent из темы
└── data/
    ├── conchy.portal
    ├── org.freedesktop.impl.portal.desktop.conchy.service
    └── conchy-portals.conf       # итоговый (каноничный) конфиг; копия уходит в conchy-wm/resources
```

`proto` не тянет ни tokio, ни gpui, ни сам `zbus` (только `zvariant` + `serde`), чтобы его без
конфликтов подключили и `conchy-files` (zbus 5, blocking), и `conchy-shell` (zbus 5.18, tokio).
`#[proxy]`-трейты к хостам живут в демоне, `#[interface]`-реализации — в каждом хосте.

### 2.2 Протокол «демон ↔ хост UI»

- Хост регистрирует на сессионной шине **отдельное имя**: `org.conchy.Portal.FilesHost`
  (conchy-files) и `org.conchy.Portal.ShellHost` (conchy-shell), путь `/org/conchy/Portal`.
- Модель вызова — **долгий pending-вызов**: демон вызывает, например,
  `FilesHost.OpenFile(id, req) -> (response u, result)`, хост показывает окно и отвечает, когда
  пользователь закончил. Плюсы: если хост упал, демон сразу получает ошибку и отвечает порталу `2`.
  - У прокси в демоне **отключить таймаут** вызова (пользователь может думать минуты).
  - Отмена: когда xdp зовёт `Request.Close` на хэндле, демон вызывает `Host.Cancel(id)`; хост
    закрывает окно и завершает pending-вызов ответом `1`.
- `id` — строка = object path хэндла запроса (уникален и удобен в логах).
- Типы запросов/ответов — структуры из `proto` (`#[derive(Serialize, Deserialize, Type)]`),
  **уже разобранные** демоном из `a{sv}` портала. Хосты не знают о формате порталов.
- Хардненинг (Фаза 7): хост принимает вызовы только от владельца имени
  `org.freedesktop.impl.portal.desktop.conchy` (проверка `header.sender()`).

### 2.3 Запуск и регистрация

- `data/org.freedesktop.impl.portal.desktop.conchy.service` (D-Bus-активация, без `SystemdService=` —
  у нас Void и нет systemd):
  ```ini
  [D-BUS Service]
  Name=org.freedesktop.impl.portal.desktop.conchy
  Exec=/usr/libexec/xdg-desktop-portal-conchy
  ```
- `data/conchy.portal`:
  ```ini
  [portal]
  DBusName=org.freedesktop.impl.portal.desktop.conchy
  Interfaces=org.freedesktop.impl.portal.Settings;   # дополнять по мере готовности фаз
  UseIn=conchy
  ```
- Хосты UI: шелл всегда запущен сессией. Для `conchy-files` нужен резидентный режим (Фаза 3):
  `conchy-files --service` — основной экземпляр без окон, не завершается после закрытия последнего
  окна; плюс D-Bus `.service`-файл для `org.conchy.Portal.FilesHost`, чтобы его поднимала активация.
- Для разработки: `just`-рецепт или скрипт `scripts/dev-install.sh`, который ставит `.portal`,
  `.service` и конфиг в `~/.local/share/...` / `~/.config/xdg-desktop-portal/` и делает
  `pkill -f xdg-desktop-portal` (фронтенд перечитает при следующей активации).
  **Важно:** xdp ищет `.portal` только в `/usr/share/xdg-desktop-portal/portals/` (или
  `$XDG_DESKTOP_PORTAL_DIR`); для dev-установки использовать переменную окружения фронтенда, проверить
  в Фазе 0, какая поддерживается установленной версией.

### 2.4 Бюджеты производительности

| Метрика | Цель |
|---|---|
| Холодный старт демона до `RequestName` | < 20 мс |
| RSS демона в простое | < 6 МБ |
| `Settings.ReadAll` | < 1 мс |
| `SettingChanged` после сохранения `theme.kdl` | < 50 мс |
| FileChooser: от вызова до первого кадра (conchy-files тёплый) | < 80 мс |
| FileChooser: то же, холодный старт conchy-files | < 300 мс |
| Access / AppChooser / пикер ScreenCast: до первого кадра | < 32 мс (2 кадра) |
| Прокрутка в пикере файлов (папка 50 000 файлов) | без пропуска кадров — та же сетка, что в conchy-files |

---

## 3. Общие правила реализации impl-порталов (прочитать до Фазы 1)

1. **Request.** Почти каждый метод получает `handle: ObjectPath`. Пока метод выполняется, демон
   **обязан** экспортировать на этом пути объект `org.freedesktop.impl.portal.Request` с методом
   `Close()`. После ответа — снять экспорт. Сделать RAII-guard в `request.rs`:
   `RequestGuard::new(server, handle) -> (guard, cancel_rx)`; `Drop` снимает объект.
2. **Session** (ScreenCast): аналогично, `org.freedesktop.impl.portal.Session` на `session_handle`,
   метод `Close()`, сигнал `Closed`, свойство `version`. Живёт до закрытия сессии.
3. **Коды ответа:** `0` — успех, `1` — пользователь отменил, `2` — другая ошибка/прервано.
4. **Строки `ay`** (`current_folder`, `current_file`, `files`) — байтовые, **с завершающим `\0`**;
   отрезать/добавлять ноль аккуратно, путь — `OsStr::from_bytes`, не `String`.
5. **parent_window**: строка `wayland:<handle>` (xdg-foreign v2) или `x11:<hex>` или пусто. В Фазах 1–6
   только логируем и прокидываем в хост; использование — Фаза 7.
6. Каждый интерфейс отдаёт свойство `version` (u) с той версией, которую реально поддерживаем.
7. Регистрация всех интерфейсов на одном пути `/org/freedesktop/portal/desktop`.
8. Неизвестные ключи в `a{sv}` игнорировать, не падать. Неверный тип известного ключа — `warn!` и
   поведение по умолчанию.
9. Тесты impl-уровня: поднять приватную шину (`dbus-daemon --session --print-address --nofork` в
   тесте) или `zbus` p2p-соединение, зарегистрировать интерфейс и звать его `#[proxy]`-клиентом.
   Тесты не должны требовать запущенного xdp/композитора.

---

## Фаза 0. Каркас и инфраструктура

**Задачи**
1. Проверить пункты «Не проверено» из §1, записать результаты в журнал.
2. Создать воркспейс по §2.1; `daemon` стартует, берёт имя
   `org.freedesktop.impl.portal.desktop.conchy`, пишет лог через `tracing-subscriber`
   (`RUST_LOG`), корректно завершается по SIGTERM.
3. `request.rs` + `session.rs` с юнит-тестами (экспорт/снятие, `Close` будит cancel-канал).
4. `data/*` файлы и `scripts/dev-install.sh`.
5. Профиль release: `lto = true`, `codegen-units = 1`, `opt-level = 3`, `panic = "abort"`, `strip = true`.
6. Добавить `conchy.portal` в список, но **пока без интерфейсов** — демон ничего не перехватывает.

**Готово, если:** `busctl --user introspect org.freedesktop.impl.portal.desktop.conchy /org/freedesktop/portal/desktop`
активирует демон через D-Bus; `cargo test` зелёный; RSS и время старта в бюджете.

---

## Фаза 1. Settings (максимум пользы на строку кода)

**Интерфейс** `org.freedesktop.impl.portal.Settings` (version 2):
`ReadAll(namespaces as) -> a{sa{sv}}`, `Read(namespace s, key s) -> v`,
сигнал `SettingChanged(namespace s, key s, value v)`.
`namespaces` поддерживает глоб с `*` в конце (`org.freedesktop.*`); пустой список = всё.

**Ключи**

| namespace | key | тип | откуда |
|---|---|---|---|
| `org.freedesktop.appearance` | `color-scheme` | `u` (0 нет предпочтения, 1 тёмная, 2 светлая) | тема |
| `org.freedesktop.appearance` | `accent-color` | `(ddd)` sRGB 0..1 | тема |
| `org.freedesktop.appearance` | `contrast` | `u` (0 обычный, 1 высокий) | конфиг (по умолчанию 0) |
| `org.freedesktop.appearance` | `reduced-motion` | `u` | только если версия xdp знает ключ; источник — анимации wm (`off` → 1) |
| `org.gnome.desktop.interface` | `color-scheme` | `s` (`prefer-dark`/`prefer-light`/`default`) | тема — для старых GTK/Flatpak |
| `org.gnome.desktop.interface` | `icon-theme`, `cursor-theme`, `cursor-size`, `font-name` | `s`/`i` | `files.kdl` (icon-theme), `wm.kdl` (cursor), шрифт — из конфига шелла, если есть |

**Источник темы**
1. Добавить в `conchy-config` модуль `schema/appearance.rs` (без gpui!):
   `fn appearance_from_theme_kdl(text: &str) -> Appearance { color_scheme, accent: [f64;3], contrast }`.
   - Тёмная/светлая: та же логика, что `is_dark()` в `conchy-shell/crates/ui/src/theme/mod.rs:38`
     (перенести/продублировать формулу, сослаться комментарием). Если в теме явно есть вариант
     (`variant "dark"`) — он главнее.
   - Акцент: `colors.accent`, если задан в `theme.kdl`, иначе `base16.base0D`.
   - Опциональный явный оверрайд: блок `appearance { color-scheme "dark"; accent "#rrggbb"; contrast "high" }`
     в `theme.kdl` (или в шелл-конфиге — выбрать по факту, записать в журнал).
2. Демон читает `theme.kdl` при старте и следит через `notify` (как `conchy-config/src/watcher.rs`),
   с дебаунсом ~30 мс; эмитит `SettingChanged` **только по изменившимся ключам**.
3. Тесты на парсер: тёмная base16, светлая base16, явный оверрайд, битый файл (→ прежнее значение, не дефолт).

**Конфиг**: `org.freedesktop.impl.portal.Settings=conchy;` (эксклюзивно, чтобы gnome-бэкенд со своими
gsettings не перебивал значения), в `conchy.portal` добавить интерфейс.

**Готово, если:**
```sh
gdbus call --session --dest org.freedesktop.portal.Desktop --object-path /org/freedesktop/portal/desktop \
  --method org.freedesktop.portal.Settings.ReadOne org.freedesktop.appearance color-scheme
```
возвращает правильное значение; смена темы в шелле → `gdbus monitor` видит `SettingChanged`;
Firefox и любое libadwaita-приложение переключаются без перезапуска.

---

## Фаза 2. Screenshot + PickColor (почти готово в композиторе)

**Интерфейс** `org.freedesktop.impl.portal.Screenshot` (version 2):
`Screenshot(handle o, app_id s, parent_window s, options a{sv}) -> (u, a{sv})` — options:
`modal b`, `interactive b`, `permission_store_checked b`; результат `uri s`.
`PickColor(handle o, app_id s, parent_window s, options a{sv}) -> (u, a{sv})` — результат `color (ddd)`.

**Реализация**
- Неинтерактивный: `org.gnome.Shell.Screenshot.Screenshot` в conchy-wm → путь файла → `file://` URI.
  Файл класть в `$XDG_RUNTIME_DIR/conchy-portal/` или `~/Pictures/Screenshots` — сверить с тем, что
  делает wm, записать решение.
- Интерактивный: запустить `conchy-shot` в новом режиме `--portal-output <path>` (выбор области,
  без аннотаций по умолчанию; Enter — сохранить в путь и выйти с кодом 0, Esc — код 1). Демон ждёт
  процесс асинхронно; `Request.Close` → SIGTERM процессу.
- PickColor: `org.gnome.Shell.Screenshot.PickColor` (уже есть в wm, `ScreenshotToNiri::PickColor`).
- Разрешение (`permission_store_checked == false` и не интерактивно) — до Фазы 5 отвечать
  разрешением; после Фазы 5 — спрашивать через Access-диалог шелла.

**Готово, если:** `ashpd`-пример (или `gdbus call` к фронтенду `org.freedesktop.portal.Screenshot`)
возвращает валидный PNG; пипетка в GIMP/Inkscape-Flatpak работает.

---

## Фаза 3. FileChooser через conchy-files (самая заметная фаза)

**Интерфейс** `org.freedesktop.impl.portal.FileChooser` (version по XML, ожидаемо 4–5):
- `OpenFile(handle, app_id, parent_window, title, options) -> (u, a{sv})`
  options: `accept_label s`, `modal b`, `multiple b`, `directory b`, `filters a(sa(us))`,
  `current_filter (sa(us))`, `choices a(ssa(ss)s)`, `current_folder ay`.
- `SaveFile(...)` options: + `current_name s`, `current_file ay`.
- `SaveFiles(...)` options: + `files aay` (выбор папки, куда сохранить несколько файлов).
- Результат: `uris as`, `choices a(ss)`, `current_filter (sa(us))`, `writable b`.
- Фильтр: `(имя, [(0, "*.png") | (1, "image/png")])` — 0 = glob, 1 = MIME.
  Для MIME использовать `conchy-files/src/mime.rs` (`MimeDb::guess_fast`/`unalias`, учитывать подклассы),
  для glob — `mime::glob_match`.

### 3a. Демон
- Разобрать `a{sv}` в `proto::FilePickRequest { kind: Open{multiple, directory} | Save{name, file} | SaveFiles{files},
  title, accept_label, filters, current_filter, choices, current_folder: Option<PathBuf>, app_id, parent_window }`.
- Вызвать `FilesHost.Pick(id, req) -> (response, FilePickResult)`; сконвертировать обратно в `a{sv}`
  (`uris` — `file://` с percent-encoding; в `conchy-files` есть `percent_decode`, нужен и encode).
- Если имени `org.conchy.Portal.FilesHost` нет — D-Bus-активация (файл `.service`), ожидание
  ≤ 2 с, иначе ответ `2`.

### 3b. conchy-files: режим пикера
1. `src/portal_host.rs` по образцу `fm1.rs`: `#[interface(name = "org.conchy.Portal.FilesHost")]`,
   методы `Pick`, `ChooseApp` (Фаза 4), `Cancel`. Метод `async`: шлёт запрос в GPUI через канал,
   ждёт `futures::channel::oneshot`.
2. Резидентный режим `--service`: основной экземпляр живёт без окон; кеши MIME/иконок прогреты.
   Сессия: добавить в автозапуск **или** только D-Bus-активация (выбрать; записать в журнал).
   Обычный запуск `conchy-files` после этого просто передаёт запрос резидентному экземпляру через
   существующий `instance.rs`.
3. `FilesWindow` получает `mode: WindowMode::{Browse, Pick(PickState)}`. В режиме Pick:
   - нижняя панель: поле имени (Save), выпадающий фильтр, `choices` (combobox/checkbox),
     кнопки «Отмена» и `accept_label` (по умолчанию «Открыть»/«Сохранить»/«Выбрать»);
   - фильтр скрывает не подходящие файлы (папки видны всегда), фильтрация на уровне модели, без
     пересоздания сетки — прокрутка должна оставаться такой же быстрой;
   - двойной клик/Enter по файлу = принять; по папке = войти; `directory=true` — принимается текущая
     или выделенная папка;
   - Save: при существующем файле — `Dialog::Confirm` («Заменить?»); `writable` = `true`;
   - Esc / закрытие окна = ответ `1`; отключить операции, опасные в пикере (удаление, корзина — оставить
     на усмотрение, но по умолчанию скрыть из контекстного меню);
   - стартовая папка: `current_folder` → папка `current_file` → последняя папка для этого `app_id`
     (хранить в `~/.local/state/conchy/files-picker.kdl`) → `$HOME`;
   - заголовок окна = `title`; `app_id` окна Wayland = `conchy-files-picker`, чтобы в `wm.kdl` можно было
     дать правило «плавающее, по центру, 900×600».
4. Добавить правило окна для `conchy-files-picker` в дефолтный конфиг wm (`conchy-wm/resources/default-config.kdl`).

**Конфиг:** `org.freedesktop.impl.portal.FileChooser=conchy;`.

**Готово, если:** Firefox «Прикрепить файл», «Сохранить как», Telegram Desktop, любое Flatpak-приложение
открывают пикер conchy-files; мультивыбор, выбор папки, фильтры, перезапись работают; тёплое
открытие < 80 мс (лог `tracing`); закрытие окна корректно отменяет запрос.

---

## Фаза 4. AppChooser через openwith.rs

**Интерфейс** `org.freedesktop.impl.portal.AppChooser`:
- `ChooseApplication(handle, app_id, parent_window, choices as, options a{sv}) -> (u, a{sv})`;
  options: `last_choice s`, `modal b`, `content_type s`, `uri s`, `filename s`, `activation_token s`;
  результат: `choice s` (desktop id **без** `.desktop`), `activation_token s`.
- `UpdateChoices(handle o, choices as)` — обновить список в открытом диалоге.

**Реализация**
- Демон → `FilesHost.ChooseApp(id, req)`; `UpdateChoices` → `FilesHost.UpdateChoices(id, choices)`.
- В conchy-files вынести диалог из `ui/openwith.rs` так, чтобы он рендерился и как оверлей окна, и как
  отдельное маленькое окно (`app_id = conchy-files-appchooser`). Показывать только приложения из
  `choices` (сопоставить с `apps::all_apps()` по id), `last_choice` — выделен, вверху; иконки — тот же
  кеш иконок.
- Чекбокс «Всегда открывать так» — **не** писать mimeapps сам: портал делает это сам через
  permission store. Не дублировать.
- Сгенерировать `activation_token` через `xdg_activation_v1`, если GPUI позволяет; если нет —
  вернуть пустым и отметить в журнале.

**Готово, если:** `xdg-open` из Flatpak / «Открыть с помощью» в Firefox показывает список Conchy,
выбор запускает приложение.

---

## Фаза 5. Access (запросы разрешений) в conchy-shell

**Интерфейс** `org.freedesktop.impl.portal.Access`:
`AccessDialog(handle, app_id, parent_window, title, subtitle, body, options) -> (u, a{sv})`;
options: `modal b`, `deny_label s`, `grant_label s`, `icon s`, `choices a(ssa(ss)s)`;
результат: `choices a(ss)`.
Этот диалог фронтенд использует для камеры, геолокации, фоновой работы, скриншотов и т.д.

**Реализация**
- В шелле: модуль `crates/app/src/portal_host/` с `#[interface(name = "org.conchy.Portal.ShellHost")]`
  (методы `Access`, `PickCastSources` — Фаза 6, `Cancel`).
- Окно — layer-shell `Layer::Overlay`, `keyboard_interactivity = exclusive`, затемнение фона,
  по центру активного выхода. Компоненты — из `crates/ui` (кнопки, чекбоксы, combobox).
- Имя и иконка приложения: по `app_id` найти `.desktop` (в шелле есть `services/applications`).
- Клавиатура: Esc = отклонить, Enter = кнопка по умолчанию (**«Запретить»**, для безопасности —
  разрешение только явным кликом/Tab+Enter).
- Очередь: несколько запросов одновременно показываются по одному.
- После Фазы 5 включить запрос разрешения в Screenshot (см. Фазу 2).

**Конфиг:** `org.freedesktop.impl.portal.Access=conchy;`.

**Готово, если:** Flatpak-приложение, запрашивающее камеру (или `gdbus call` к `Camera.AccessCamera`),
показывает диалог шелла; ответ сохраняется в permission store (повторный запрос не показывается).

---

## Фаза 6. ScreenCast (самая сложная)

**До завершения фазы держать** `org.freedesktop.impl.portal.ScreenCast=gnome;` — xdg-desktop-portal-gnome
уже работает поверх нашей Mutter-эмуляции. Переключить на `conchy` только после «Готово, если».

**Интерфейс** `org.freedesktop.impl.portal.ScreenCast` (version по XML, ожидаемо 5):
- `CreateSession(handle, session_handle, app_id, options) -> (u, a{sv})`
- `SelectSources(handle, session_handle, app_id, options) -> (u, a{sv})`; options: `types u`
  (1 монитор, 2 окно, 4 виртуальный), `multiple b`, `cursor_mode u` (1 скрыт, 2 встроен, 4 метаданные),
  `restore_data (suv)`, `persist_mode u` (0 нет, 1 пока жив процесс, 2 навсегда).
- `Start(handle, session_handle, app_id, parent_window, options) -> (u, a{sv})`; результат:
  `streams a(ua{sv})` (node_id PipeWire + `position (ii)`, `size (ii)`, `source_type u`, `id s`),
  `persist_mode u`, `restore_data (suv)`.
- Свойства: `AvailableSourceTypes u` (= 1|2, виртуальный — позже), `AvailableCursorModes u`, `version u`.

**Реализация**
1. `screencast.rs`: состояние сессии (`HashMap<OwnedObjectPath, CastSession>`), объект Session из §3.
2. `SelectSources` только запоминает параметры (UI здесь **не** показываем — так делает и gnome).
3. `Start`:
   - если есть валидный `restore_data` и источник ещё существует — пропустить пикер;
   - иначе `ShellHost.PickCastSources(id, req{types, multiple, app_id}) -> (response, Vec<Source>)`,
     где `Source = Monitor{connector} | Window{id}`;
   - для каждого источника через прокси к conchy-wm: `org.gnome.Mutter.ScreenCast.CreateSession` →
     `Session.RecordMonitor(connector, {cursor-mode})` / `RecordWindow({window-id, cursor-mode})` →
     `Session.Start()` → дождаться сигнала `Stream.PipeWireStreamAdded(node_id)` (таймаут 3 с);
   - собрать `streams`, вернуть.
   - Закрытие: `Session.Close` от xdp → `Mutter Session.Stop`; сигнал Mutter `Closed` → наш `Closed`.
4. Пикер в шелле: overlay со вкладками «Экраны» / «Окна»; список выходов — из сервиса композитора
   шелла (`crates/services/src/compositor`), окна — оттуда же (заголовок, иконка приложения).
   Превью: первая версия — без живых превью (иконка + заголовок + размеры); превью окон —
   отдельная подзадача позже (через conchy-ipc-снимок или кадр из PipeWire), не блокирует фазу.
   **Проверить** соответствие id окон conchy-ipc ↔ Introspect (см. §1).
5. Restore tokens: `restore_data = ("conchy", 1, a{sv}{ "kind": "monitor"|"window", "connector": s,
   "app_id": s, "title": s })`. Для `persist_mode=2` — хранит сам фронтенд в permission store; нам
   нужно только уметь разобрать свои данные и найти источник (монитор — по connector, окно — по app_id,
   при неоднозначности — показать пикер).
6. Курсор: прокинуть `cursor_mode` в Mutter `cursor-mode` (Hidden=0/Embedded=1/Metadata=2 —
   **маппинг значений портала и Mutter отличается**, см. `CursorMode` в `mutter_screen_cast.rs`).

**Конфиг:** `org.freedesktop.impl.portal.ScreenCast=conchy;`.

**Готово, если:** демонстрация экрана и отдельного окна работает в Firefox (Google Meet / webrtc test),
Chromium, OBS (PipeWire source), Discord/Vesktop; повторный запуск OBS с сохранённым источником не
показывает пикер; закрытие трансляции из приложения и из шелла корректно гасит поток в wm.

---

## Фаза 7. Полировка и полное избавление от GTK/Qt-порталов

Каждая подзадача независима, делать по одной:
1. **Notification** → `org.freedesktop.impl.portal.Notification` как прокси к
   `org.freedesktop.Notifications` шелла (`AddNotification`/`RemoveNotification`, сигнал `ActionInvoked`).
2. **Inhibit** → idle-inhibit через `org.freedesktop.ScreenSaver` в conchy-wm (`freedesktop_screensaver.rs`).
3. **Wallpaper** → `SetWallpaperURI` → сервис обоев шелла (`crates/services/src/wallpaper`) с
   Access-подтверждением.
4. **parent_window / модальность**: `wayland:<handle>` — импортировать через xdg-foreign v2 и сделать
   окно пикера дочерним. GPUI этого, скорее всего, не умеет → вариант: новая команда `conchy-ipc`
   «сделай окно с app_id/pid X дочерним для экспортированного handle Y», реализация в conchy-wm.
   Сначала исследование, потом решение в журнал.
5. Хардненинг хостов (проверка отправителя, §2.2).
6. Превью окон в пикере ScreenCast.
7. Пакетирование (`xbps-src` шаблон): `xdg-desktop-portal-conchy` ставит бинарь, `.portal`, `.service`,
   `conchy-portals.conf`; из зависимостей сессии убрать `xdg-desktop-portal-gnome`; `-gtk` оставить
   только ради Print (или тоже убрать, если печать не нужна).

**Итоговый `conchy-portals.conf`:**
```ini
[preferred]
default=conchy;gtk;
org.freedesktop.impl.portal.Settings=conchy;
org.freedesktop.impl.portal.FileChooser=conchy;
org.freedesktop.impl.portal.AppChooser=conchy;
org.freedesktop.impl.portal.Access=conchy;
org.freedesktop.impl.portal.Screenshot=conchy;
org.freedesktop.impl.portal.ScreenCast=conchy;
org.freedesktop.impl.portal.Notification=conchy;
org.freedesktop.impl.portal.Inhibit=conchy;
org.freedesktop.impl.portal.Wallpaper=conchy;
org.freedesktop.impl.portal.Secret=oo7-portal;gnome-keyring;
org.freedesktop.impl.portal.Print=gtk;
```
Держать в синхроне `conchy-portal/data/conchy-portals.conf` и `conchy-wm/resources/conchy-portals.conf`
(в conchy-os есть копия `conchy-os/compositor/resources/` — уточнить, какая каноничная).

---

## 4. Порядок работы исполнителя в каждой фазе

1. Прочитать XML нужного impl-интерфейса в `/usr/share/dbus-1/interfaces/`.
2. Прочитать файлы из §1, которых касается фаза.
3. Реализовать демонскую часть + тесты impl-уровня (§3 п.9).
4. Реализовать хост-часть (если есть).
5. Обновить `conchy.portal` и `conchy-portals.conf`.
6. Прогнать «Готово, если» вручную, записать замеры в журнал.
7. `cargo clippy --all-targets`, `cargo test` во всех затронутых крейтах.
8. Не трогать `smithay/` и рендер wm без явной нужды; изменения в conchy-wm — только D-Bus/IPC.

## 5. Риски

| Риск | Что делать |
|---|---|
| Версия xdp старая, нет `accent-color`/`ReadOne` | Ключ всё равно отдаём; отметить в журнале, какие клиенты его не увидят |
| GPUI не умеет xdg-foreign / xdg-activation | Фаза 7 п.4 через wm-IPC; до этого — правила окон по app_id |
| Две версии gpui (`conchy-files` 0.2.2 с crates.io, шелл — git rev) | `proto` не зависит от gpui — конфликта нет; не пытаться объединять в рамках этого плана |
| id окон шелла ≠ id Introspect | Пикер возвращает id из Introspect: шелл запрашивает `GetWindows` сам |
| conchy-files холодный старт > 300 мс | Резидентный `--service` + прогрев кешей |
| Падение хоста во время диалога | pending-вызов вернёт ошибку → ответ `2`; демон живёт дальше |

## 6. Журнал решений

_(заполняет исполнитель: дата, фаза, решение/расхождение, замеры)_

-
- 2026-11-19 · Фаза 0 · Проверено:
  - `xdg-desktop-portal` 1.22.1 (`/usr/lib/xdg-desktop-portal`, на Arch не `libexec`): знает `accent-color`, `contrast`, `reduced-motion`. `.portal` ищется в `$XDG_DESKTOP_PORTAL_DIR` (строка есть в бинарнике) — dev-install и e2e-скрипты используют её.
  - XML impl-интерфейсов лежат в `/usr/share/dbus-1/interfaces/org.freedesktop.impl.portal.*.xml`.
  - id окон: `Introspect.GetWindows` ключует `mapped.id().get()` (`conchy-wm/src/wm.rs:2481`) — тот же `u64`, что `Window.id` в `conchy-ipc`. **Совпадает.** `app-id` в Introspect — с суффиксом `.desktop`. В списке есть фиктивное окно «niri Dynamic Cast Target» при фиче `xdp-gnome-screencast`.
  - Каркас шины в шелле: `conchy-shell/crates/services/src/bus.rs` (общие `session()`/`system()` соединения без потока zbus-executor) — используем в ShellHost.
  - Канонична копия `conchy-wm/resources/conchy-portals.conf`; `conchy-os/compositor` — git-submodule на conchy-wm, отдельной копии нет.
  - Замеры (release): активация D-Bus 11 мс, `startup_ms` 1.5, RSS 4.7 МБ — в бюджете.
  - Изолированные проверки: `scripts/private-bus.sh`, `scripts/e2e-env.sh` (приватная шина + настоящий xdp с `XDG_CURRENT_DESKTOP=conchy`) — запуск не требует Conchy-сессии (на dev-машине Hyprland).
- 2026-11-19 · Фаза 1 · Решения:
  - Явные оверрайды (`appearance { color-scheme/accent/contrast/reduced-motion }`) — в **отдельном `~/.config/conchy/appearance.kdl`**, а не в `theme.kdl`: `StoredTheme` шелла — `deny_unknown_fields` и переписывается целиком при сохранении темы. Ключ `variant` в `theme.kdl` поэтому тоже не используется.
  - Формула тёмная/светлая: HSL-светлота `base00 < 0.5` (`Base16Palette::appearance()` в `conchy-shell/crates/ui/src/theme/base16.rs`, а не `mod.rs:38`, где лишь enum). Парсер — `conchy-config/src/schema/appearance.rs` (ветка `feat/appearance-schema`).
  - `font-name` не отдаём: в шелле нет настройки семейства шрифта (только `font_size`).
  - `cursor-theme/size` — из блока `cursor { xcursor-theme; xcursor-size }` в `wm.kdl`; `reduced-motion` = `animations { off }` в `wm.kdl` или оверрайд.
  - Замер: `SettingChanged` после записи `theme.kdl` — перезагрузка 1.6 мс + дебаунс 30 мс (e2e через xdp).
  - Проверено через настоящий xdp: `ReadOne`, `Read`, `SettingChanged`. Firefox/libadwaita на dev-машине (Hyprland) не проверялись.
- 2026-11-19 · Фаза 2 · Решения:
  - Неинтерактивный снимок: `org.gnome.Shell.Screenshot.Screenshot` в conchy-wm; параметр `filename` wm игнорирует и пишет в настроенный каталог скриншотов (`make_screenshot_path`), демон отдаёт именно этот путь как `file://` URI (копирования нет).
  - Интерактивный/`target=Area`: `conchy-shot area --portal-output <файл>` (новый флаг, ветка `feat/portal-output` в conchy-shot): сохраняет только PNG, без буфера обмена и уведомления; код 0 — сохранено, 1 — отмена. Файлы кладутся в `$XDG_RUNTIME_DIR/conchy-portal/`. Бинарь переопределяется `CONCHY_SHOT_BIN`.
  - `Request.Close` во время `conchy-shot` → `kill_on_drop` (SIGKILL, а не SIGTERM как в плане: для layer-shell-клиента разницы нет, зато нет гонки).
  - `AvailableTargets = 1|4` (экран + область); окно/активное окно не поддержаны. Версия интерфейса 3 (по XML в системе).
  - PickColor: «no color picked» от wm → ответ `1` (отмена), прочие ошибки → `2`.
  - Разрешение для неинтерактивного снимка: **фронтенд xdp сам** зовёт `impl.Access` (диалог «разрешить скриншот?») и только потом бэкенд Screenshot. Значит, до Фазы 5 это делает gtk-Access (на нашем conf `Access=gtk`), а флаг `permission_store_checked` демону проверять не нужно. Пункт плана «после Фазы 5 спрашивать через Access-диалог шелла» выполняется автоматически переключением `Access=conchy`.
  - e2e: `scripts/e2e/screenshot.py` через настоящий xdp (интерактивный снимок, отмена, PickColor) — зелёный.
- 2026-11-19 · Фаза 3 · Решения и замеры:
  - **GPUI останавливает цикл событий, когда на Wayland не осталось окон** (`gpui-0.2.2/src/platform/linux/wayland/client.rs:387`) — «вечного» окна-less резидента нет. Решение: по умолчанию хост поднимается D-Bus-активацией (`org.conchy.Portal.FilesHost.service` → `conchy-files --service`) на каждый запрос, процесс выходит после закрытия окна (100 мс паузы на выходе, чтобы ответ D-Bus долетел). Для тёплого режима есть `conchy-files --resident` с невидимым окном-якорем `conchy-files-keepalive` (+правило wm; `show:false` GPUI на Wayland игнорирует, на Hyprland окно видно как тайл — поэтому режим не по умолчанию), выход при простое 15 мин (`CONCHY_FILES_IDLE_SECS`).
  - Хост занимает имя `org.conchy.Portal.FilesHost` **до** `Application::new()` (инициализация GPUI долгая), запросы копятся в канале — иначе активация не укладывалась в 2 с.
  - `FilePickResult` несёт сырые пути (`paths: aay`), `file://` с percent-encoding собирает демон (`uri.rs`), поэтому encode в conchy-files не понадобился.
  - SaveFiles: к выбранной папке дописываются имена файлов; занятое имя получает суффикс `a (1).txt` без диалога (спецификация это разрешает).
  - `writable` в ответе всегда `true` (как у xdg-desktop-portal-gnome: иначе песочница не сможет писать в файл, выбранный в «Открыть»).
  - Маски фильтров сравниваются без учёта регистра; MIME — с подклассами: в `MimeDb` добавлен `is_a` (по `mime/subclasses` + правила `text/*`, `*+xml`, `octet-stream`).
  - Пикер скрывает файлы фильтром на уровне `rebuild_visible` (папки видны всегда; в режиме «папка» файлы скрыты). Операции изменения файлов в пикере блокируются (`run_cmd`), из контекстного меню остаются выбор/навигация/«создать папку».
  - Нет клиентского таймаута у вызова `Pick`; ограничение даёт dbus-daemon (`reply_timeout` шины, по умолчанию 5 мин на session bus не стоит, но зависит от конфига шины).
  - `Request.Close` → `FilesHost.Cancel(id)` → окно закрывается, ответ `1`.
  - Последняя папка приложения: `~/.local/state/conchy/files-picker.kdl` (`folder "app.id" "/путь"`, не больше 64 записей).
  - Бюджеты: первый кадр при холодном старте (debug-сборка, включая активацию) ~0.77 с; release и тёплый режим не измерялись (release gpui не собирал). e2e `scripts/e2e/filechooser.py` (xdp+демон+conchy-files, клавиши через `hyprctl dispatch hl.dsp.send_shortcut`): выбор с фильтром, Esc, Save, перезапись — зелёный.
  - Ветки: conchy-files `feat/portal-picker`, conchy-wm `feat/portal-conchy` (правила окон + conf), conchy-shot `feat/portal-output`, conchy-config `feat/appearance-schema`. conchy-files зависит от `../conchy-portal/crates/proto` по path.
