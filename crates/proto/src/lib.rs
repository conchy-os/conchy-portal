//! Протокол «демон портала ↔ хост UI».
//!
//! Только типы и константы: крейт не тянет ни `zbus`, ни `tokio`, ни `gpui`, поэтому его без
//! конфликтов версий подключают и демон, и `conchy-files`, и `conchy-shell`.

/// Имя демона на сессионной шине (то, что ищет `xdg-desktop-portal` по `.portal`-файлу).
pub const DAEMON_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.conchy";
/// Единый путь всех impl-интерфейсов портала.
pub const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

/// Общий путь объекта хоста UI.
pub const HOST_OBJECT_PATH: &str = "/org/conchy/Portal";
/// Имя хоста файлового менеджера (FileChooser, AppChooser).
pub const FILES_HOST_BUS_NAME: &str = "org.conchy.Portal.FilesHost";
/// Имя хоста шелла (Access, пикер ScreenCast).
pub const SHELL_HOST_BUS_NAME: &str = "org.conchy.Portal.ShellHost";

/// Коды ответа impl-порталов (`response`).
pub mod response {
    /// Успех.
    pub const OK: u32 = 0;
    /// Пользователь отменил.
    pub const CANCELLED: u32 = 1;
    /// Другая ошибка или прервано.
    pub const OTHER: u32 = 2;
}

/// Интерфейс хоста файлового менеджера (`org.conchy.Portal.FilesHost`).
pub const FILES_HOST_INTERFACE: &str = "org.conchy.Portal.FilesHost";
/// Интерфейс хоста шелла (`org.conchy.Portal.ShellHost`).
pub const SHELL_HOST_INTERFACE: &str = "org.conchy.Portal.ShellHost";

/// Типы запросов FileChooser. Плоские структуры (без enum с данными): так сигнатуры D-Bus
/// остаются простыми и стабильными, а хосту не нужно знать формат `a{sv}` порталов.
pub mod files {
    use serde::{Deserialize, Serialize};
    use zvariant::Type;

    /// `FilePickRequest::kind`: открыть файл(ы) или папку.
    pub const KIND_OPEN: u32 = 0;
    /// Сохранить один файл.
    pub const KIND_SAVE: u32 = 1;
    /// Выбрать папку для сохранения нескольких файлов.
    pub const KIND_SAVE_FILES: u32 = 2;

    /// `FilterRule::kind`: маска имени (`*.png`).
    pub const RULE_GLOB: u32 = 0;
    /// MIME-тип (`image/png`).
    pub const RULE_MIME: u32 = 1;

    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
    pub struct FilterRule {
        pub kind: u32,
        pub pattern: String,
    }

    /// Фильтр файлов: `(имя, [(0|1, шаблон)])` — как `(sa(us))` в портале.
    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
    pub struct Filter {
        pub name: String,
        pub rules: Vec<FilterRule>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
    pub struct ChoiceOption {
        pub id: String,
        pub label: String,
    }

    /// Дополнительный выбор в диалоге: `(id, подпись, варианты, начальное значение)`.
    /// Пустой список вариантов — чекбокс (`"true"`/`"false"`), иначе выпадающий список.
    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
    pub struct Choice {
        pub id: String,
        pub label: String,
        pub options: Vec<ChoiceOption>,
        pub initial: String,
    }

    /// Выбранное значение `Choice`: `(id, значение)`.
    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
    pub struct ChoiceSelection {
        pub id: String,
        pub value: String,
    }

    /// Разобранный демоном вызов `OpenFile`/`SaveFile`/`SaveFiles`.
    /// Пути — байты без завершающего нуля (демон его уже отрезал).
    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
    pub struct FilePickRequest {
        pub kind: u32,
        pub app_id: String,
        pub parent_window: String,
        pub title: String,
        pub accept_label: String,
        pub modal: bool,
        pub multiple: bool,
        pub directory: bool,
        pub filters: Vec<Filter>,
        /// Ноль или один элемент (D-Bus не знает `Option`).
        pub current_filter: Vec<Filter>,
        pub choices: Vec<Choice>,
        pub current_folder: Vec<u8>,
        pub current_name: String,
        pub current_file: Vec<u8>,
        pub files: Vec<Vec<u8>>,
    }

    /// Ответ хоста: что выбрал пользователь (`response` приходит отдельно).
    #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
    pub struct FilePickResult {
        /// Пути выбранных элементов — байты без нуля; демон превращает их в `file://` URI.
        pub paths: Vec<Vec<u8>>,
        pub choices: Vec<ChoiceSelection>,
        pub current_filter: Vec<Filter>,
        pub writable: bool,
    }
}
