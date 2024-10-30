use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::fs::{read_to_string, write};
use ui::{ComponentHandle, GlobalState, MainWindow};

#[derive(Serialize, Deserialize, Default)]
pub struct Settings {
    path_32: String,
    path_64: String,
}

pub async fn apply_saved_settings<'a>(state: &GlobalState<'a>) {
    // let state = app.global::<GlobalState>();
    state.set_settings(Settings::load().await.into());
}

pub fn save_settings(app: &MainWindow) {
    let state = app.global::<GlobalState>();
    let settings: Settings = state.get_settings().into();
    tokio::spawn(async move { settings.save().await });
}

impl Settings {
    pub async fn load() -> Self {
        toml::from_str(&read_to_string("conf.toml").await.unwrap_or_default()).unwrap_or_default()
    }

    pub async fn save(&self) -> Result<()> {
        Ok(write("conf.toml", toml::to_string_pretty(self)?).await?)
    }
}

impl From<ui::Settings> for Settings {
    fn from(settings: ui::Settings) -> Self {
        Self {
            path_32: settings.path_32.into(),
            path_64: settings.path_64.into(),
        }
    }
}

impl From<Settings> for ui::Settings {
    fn from(val: Settings) -> ui::Settings {
        ui::Settings {
            path_32: val.path_32.into(),
            path_64: val.path_64.into(),
        }
    }
}
