use client_api::api::settings::PublicSettings;
use dioxus::prelude::*;

fn distribution_setting_is_enabled(value: Option<bool>) -> bool {
    matches!(value, Some(true))
}

#[derive(Clone, Default)]
pub struct PublicSettingsState {
    pub settings: Option<PublicSettings>,
    pub loaded: bool,
}

#[derive(Clone, Copy)]
pub struct PublicSettingsStore {
    pub state: Signal<PublicSettingsState>,
}

impl PublicSettingsStore {
    pub fn new(state: Signal<PublicSettingsState>) -> Self {
        Self { state }
    }

    pub fn loaded(&self) -> bool {
        self.state.read().loaded
    }

    pub fn site_name(&self) -> Option<String> {
        self.state
            .read()
            .settings
            .as_ref()
            .map(|settings| settings.site_name.clone())
    }

    pub fn site_logo_url(&self) -> Option<String> {
        self.state
            .read()
            .settings
            .as_ref()
            .and_then(|settings| settings.site_logo_url.as_deref())
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
    }

    pub fn distribution_enabled(&self) -> Option<bool> {
        self.state
            .read()
            .settings
            .as_ref()
            .map(|settings| settings.distribution_enabled)
    }

    /// Fail closed until the public settings request explicitly reports that
    /// distribution is enabled.
    pub fn distribution_is_enabled(&self) -> bool {
        distribution_setting_is_enabled(self.distribution_enabled())
    }

    pub fn set(&mut self, settings: PublicSettings) {
        *self.state.write() = PublicSettingsState {
            settings: Some(settings),
            loaded: true,
        };
    }

    pub fn mark_loaded(&mut self) {
        self.state.write().loaded = true;
    }

    pub fn set_distribution_enabled(&mut self, enabled: bool) {
        let mut state = self.state.write();
        state.loaded = true;
        let settings = state.settings.get_or_insert_with(PublicSettings::default);
        settings.distribution_enabled = enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::distribution_setting_is_enabled;

    #[test]
    fn distribution_visibility_requires_an_explicit_enabled_setting() {
        assert!(distribution_setting_is_enabled(Some(true)));
        assert!(!distribution_setting_is_enabled(Some(false)));
        assert!(!distribution_setting_is_enabled(None));
    }
}
