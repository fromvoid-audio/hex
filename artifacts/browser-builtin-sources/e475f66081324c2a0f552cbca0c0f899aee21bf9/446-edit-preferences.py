from pathlib import Path
p=Path('crates/hex-preferences/src/lib.rs');s=p.read_text()
s=s.replace('/// Additional browser folders, in user-visible order.','''/// Stable identities independent of the user's current Home path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserBuiltin {
    Home,
    Music,
    Samples,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuiltinPreference {
    source: Option<BrowserBuiltin>,
    hidden: bool,
    icon: Option<String>,
}

/// Additional browser folders, in user-visible order.''')
s=s.replace('    icons: BTreeMap<PathBuf, String>,','    icons: BTreeMap<PathBuf, String>,\n    builtins: BTreeMap<BrowserBuiltin, BuiltinPreference>,\n    hidden_places: HashSet<PathBuf>,',1)
s=s.replace('            icons: BTreeMap::new(),','            icons: BTreeMap::new(),\n            builtins: BTreeMap::new(),\n            hidden_places: HashSet::new(),',1)
needle='    pub fn additional(&self) -> &[PathBuf] {'
s=s.replace(needle,'''    pub fn builtin_icon(&self, source: BrowserBuiltin) -> Option<&str> {
        self.builtins.get(&source).and_then(|value| value.icon.as_deref())
    }

    pub fn builtin_hidden(&self, source: BrowserBuiltin) -> bool {
        self.builtins.get(&source).is_some_and(|value| value.hidden)
    }

    pub fn set_builtin_icon(&mut self, source: BrowserBuiltin, icon: Option<&str>) -> Result<(), PreferencesError> {
        if let Some(id) = icon { validate_icon_id(id)?; }
        self.builtins.entry(source).or_default().icon = icon.map(str::to_owned);
        Ok(())
    }

    pub fn set_builtin_hidden(&mut self, source: BrowserBuiltin, hidden: bool) {
        self.builtins.entry(source).or_default().hidden = hidden;
    }

    pub fn place_hidden(&self, path: &Path) -> bool {
        self.hidden_places.contains(path)
    }

    pub fn set_place_hidden(&mut self, path: &Path, hidden: bool) -> Result<(), PreferencesError> {
        if !self.additional.iter().any(|place| place == path) {
            return Err(PreferencesError::InvalidDocument("hidden source has no matching place".into()));
        }
        if hidden { self.hidden_places.insert(path.to_owned()); }
        else { self.hidden_places.remove(path); }
        Ok(())
    }

'''+needle)
start=s.index('            if id.is_empty()',s.index('pub fn set_icon'))
end=s.index('            self.icons.insert',start)
s=s[:start]+'            validate_icon_id(id)?;\n'+s[end:]
needle='/// Recently opened project files, most recent first.'
s=s.replace(needle,'''fn validate_icon_id(id: &str) -> Result<(), PreferencesError> {
    if id.is_empty() || id.len() > MAX_PLACE_ICON_ID_BYTES || id.chars().any(char::is_control) {
        return Err(PreferencesError::InvalidDocument("invalid place icon identifier".into()));
    }
    Ok(())
}

'''+needle)
needle='    Ok(places)\n}\n\n/// Load recent'
s=s.replace(needle,'''    if document.builtin_sources.len() > 3 || document.hidden_places.len() > MAX_USER_PLACES {
        return Err(PreferencesError::InvalidDocument("too many source preferences".into()));
    }
    for mut value in document.builtin_sources {
        let source = value.source.take().ok_or_else(|| PreferencesError::InvalidDocument("missing builtin identity".into()))?;
        if let Some(id) = &value.icon { validate_icon_id(id)?; }
        if places.builtins.insert(source, value).is_some() {
            return Err(PreferencesError::InvalidDocument("duplicate builtin identity".into()));
        }
    }
    for stored in document.hidden_places {
        let path = stored.into_path()?;
        if places.place_hidden(&path) {
            return Err(PreferencesError::InvalidDocument("duplicate hidden place".into()));
        }
        places.set_place_hidden(&path, true)?;
    }
    Ok(places)
}

/// Load recent''')
s=s.replace('        version: DOCUMENT_VERSION,\n        place_icons:', '''        version: DOCUMENT_VERSION,
        builtin_sources: places.builtins.iter().map(|(source, value)| BuiltinPreference {
            source: Some(*source), hidden: value.hidden, icon: value.icon.clone(),
        }).collect(),
        hidden_places: places.additional.iter().filter(|path| places.place_hidden(path))
            .map(|path| StoredPath::from_path(path)).collect::<Result<Vec<_>, _>>()?,
        place_icons:''')
s=s.replace('    place_icons: Vec<StoredPlaceIcon>,\n}', '''    place_icons: Vec<StoredPlaceIcon>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    builtin_sources: Vec<BuiltinPreference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden_places: Vec<StoredPath>,
}''')
p.write_text(s)
