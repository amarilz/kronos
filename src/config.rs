use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

const DATABASE_PATH_KEY: &str = "database.path";

pub fn properties_file() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Could not locate the user's configuration directory")?
        .join("kronos")
        .join("kronos.properties"))
}

pub fn load_database_path() -> Result<Option<PathBuf>> {
    load_database_path_from(&properties_file()?)
}

fn load_database_path_from(properties_path: &Path) -> Result<Option<PathBuf>> {
    if !properties_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(properties_path)
        .with_context(|| format!("Could not read {}", properties_path.display()))?;
    let properties = parse_properties(&contents)
        .with_context(|| format!("Could not parse {}", properties_path.display()))?;
    properties
        .get(DATABASE_PATH_KEY)
        .map(|value| validate_database_path(value))
        .transpose()
}

fn validate_database_path(value: &str) -> Result<PathBuf> {
    let configured_path = PathBuf::from(value);
    if !configured_path.is_absolute() {
        bail!(
            "The saved database path is not absolute: {}",
            configured_path.display()
        );
    }
    Ok(configured_path)
}

pub fn save_database_path(database_path: &Path) -> Result<()> {
    if !database_path.is_absolute() {
        bail!("The database path must be absolute");
    }
    save_database_path_to(&properties_file()?, database_path)
}

fn save_database_path_to(properties_path: &Path, database_path: &Path) -> Result<()> {
    let mut properties = if properties_path.exists() {
        let contents = fs::read_to_string(properties_path)
            .with_context(|| format!("Could not read {}", properties_path.display()))?;
        parse_properties(&contents)
            .with_context(|| format!("Could not parse {}", properties_path.display()))?
    } else {
        BTreeMap::new()
    };
    properties.insert(
        DATABASE_PATH_KEY.to_owned(),
        database_path.to_string_lossy().into_owned(),
    );

    write_properties(properties_path, &properties)
}

fn write_properties(properties_path: &Path, properties: &BTreeMap<String, String>) -> Result<()> {
    let parent = properties_path
        .parent()
        .context("Configuration file has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("Could not create {}", parent.display()))?;

    let temp_path = properties_path.with_extension("tmp");
    let mut file = File::create(&temp_path)?;
    writeln!(file, "# Kronos settings")?;
    for (key, value) in properties {
        writeln!(file, "{key}={}", escape_property_value(value))?;
    }
    file.sync_all()?;
    drop(file);

    replace_file(&temp_path, properties_path)?;

    #[cfg(unix)]
    File::open(parent)?.sync_all()?;

    Ok(())
}

fn parse_properties(contents: &str) -> Result<BTreeMap<String, String>> {
    let mut properties = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }

        let separator = line
            .char_indices()
            .find(|(index, ch)| {
                if *ch == '\\' {
                    return false;
                }
                let preceding_backslashes = line[..*index]
                    .chars()
                    .rev()
                    .take_while(|previous| *previous == '\\')
                    .count();
                preceding_backslashes % 2 == 0 && (*ch == '=' || *ch == ':' || ch.is_whitespace())
            })
            .map(|(index, _)| index)
            .with_context(|| format!("Invalid property on line {}", index + 1))?;

        let key = line[..separator].trim();
        if key.is_empty() {
            bail!("Property key cannot be empty on line {}", index + 1);
        }

        let mut value = line[separator..].trim_start();
        if value.starts_with('=') || value.starts_with(':') {
            value = value[1..].trim_start();
        }
        properties.insert(key.to_owned(), unescape_property_value(value)?);
    }
    Ok(properties)
}

fn escape_property_value(value: &str) -> String {
    let mut escaped = String::new();
    for (index, ch) in value.chars().enumerate() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{000c}' => escaped.push_str("\\f"),
            ' ' if index == 0 => escaped.push_str("\\ "),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn unescape_property_value(value: &str) -> Result<String> {
    let mut units = Vec::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            units.extend(ch.encode_utf16(&mut [0; 2]).iter().copied());
            continue;
        }

        let escaped = chars.next().context("Property value ends with an escape")?;
        match escaped {
            'n' => units.push('\n' as u16),
            'r' => units.push('\r' as u16),
            't' => units.push('\t' as u16),
            'f' => units.push('\u{000c}' as u16),
            'u' => {
                let digits: String = chars.by_ref().take(4).collect();
                if digits.len() != 4 {
                    bail!("Invalid Unicode escape in property value");
                }
                units.push(
                    u16::from_str_radix(&digits, 16)
                        .context("Invalid Unicode escape in property value")?,
                );
            }
            other => units.extend(other.encode_utf16(&mut [0; 2]).iter().copied()),
        }
    }
    String::from_utf16(&units).context("Property value contains invalid UTF-16")
}

fn replace_file(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

        let source: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();

        if !to.exists() {
            return fs::rename(from, to)
                .with_context(|| format!("Could not create {}", to.display()));
        }

        let result = unsafe {
            ReplaceFileW(
                destination.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };

        if result == 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("Could not replace {}", to.display()));
        }

        Ok(())
    }

    #[cfg(not(windows))]
    {
        fs::rename(from, to)
            .with_context(|| format!("Could not update {}", to.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn properties_round_trip_multiple_settings_and_escaped_values() {
        let contents = "# settings\n\
                        database.path=C:\\\\Users\\\\A User\\\\kronos.sqlite3\n\
                        theme=dark\n\
                        title=Kronos\n";
        let properties = parse_properties(contents).unwrap();
        assert_eq!(
            properties.get(DATABASE_PATH_KEY).unwrap(),
            "C:\\Users\\A User\\kronos.sqlite3"
        );
        assert_eq!(properties.get("theme").unwrap(), "dark");
        assert_eq!(properties.get("title").unwrap(), "Kronos");
        assert_eq!(
            unescape_property_value(&escape_property_value("C:\\Users\\A User\\kronos.sqlite3"))
                .unwrap(),
            "C:\\Users\\A User\\kronos.sqlite3"
        );
    }

    #[test]
    fn saves_and_loads_database_path_without_losing_other_settings() {
        let directory = std::env::temp_dir().join(format!(
            "kronos-config-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let properties_path = directory.join("kronos.properties");
        let database_path = directory.join("kronos.sqlite3");

        assert_eq!(load_database_path_from(&properties_path).unwrap(), None);
        save_database_path_to(&properties_path, &database_path).unwrap();
        let mut contents = fs::read_to_string(&properties_path).unwrap();
        contents.push_str("theme=dark\n");
        fs::write(&properties_path, contents).unwrap();

        let new_path = directory.join("other.sqlite3");
        save_database_path_to(&properties_path, &new_path).unwrap();
        assert_eq!(
            load_database_path_from(&properties_path).unwrap(),
            Some(new_path)
        );
        let properties = fs::read_to_string(properties_path).unwrap();
        assert!(properties.contains("theme=dark"));
        fs::remove_dir_all(directory).unwrap();
    }
}
