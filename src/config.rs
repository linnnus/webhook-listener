use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io;
use std::fmt::{self, Display};
use serde::Deserialize;

/// All the application configuration is stored in this structure.
#[derive(PartialEq, Clone, Debug)]
pub struct Config {
    /// The secret string shared with GitHub that is used to verify signed requests.
    pub secret: String,

    /// Event-command pairs. Each element of this array should be matched (and optionally executed)
    /// against the commands in gaide.
    pub commands: Vec<Command>,
}

impl Config {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let raw_config = RawConfig::from_path(path)?;
        let secret = fs::read_to_string(raw_config.secret_file)?;
        Ok(Config {
            secret,
            commands: raw_config.commands,
        })
    }
}

/// This struct reflects the actual JSON on disk. It is further processed before being returned to
/// the rest of the application.
#[derive(Deserialize, Clone, Debug, PartialEq)]
struct RawConfig {
    /// Path to file containing the secret that was shared with GitHub.
    secret_file: PathBuf,

    /// Event-command pairs.
    commands: Vec<Command>,
}

/// Represents an event-command pair. The command is run whenever the given event is received from
/// GitHub's API.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Command {
    /// The name of an event from the GitHub API. A full list of events can be found in [GitHub's
    /// documenation][gh-events].
    ///
    /// [gh-events]: https://docs.github.com/en/webhooks/webhook-events-and-payloads
    pub event: String,

    /// Path to the program to be executed when [`event`](event) occurs.
    pub command: String,

    /// Additional arguments to bass to [`command`](command).
    #[serde(default)]
    pub args: Vec<String>,
}

/*
/// Serde helper which disallows empty strings for [`PathBuf`s](std::path::PathBuf). Based on [this
/// StackOverflow post][so].
///
/// [so]: https://stackoverflow.com/a/46755370
fn string_as_nonempty_pathbuf<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>
{
    let raw: &str = Deserialize::deserialize(deserializer)?;
    if raw.is_empty() {
        Err(de::Error::custom("path cannot be empty"))
    } else {
        Ok(PathBuf::from(raw))
    }
}
*/

/// Errors that can occur when reading configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// An IO error occured, such as failing to read the file.
    Io(io::Error),
    /// Decoding the file failed, e.g. if JSON is missing comma.
    SerdeError(serde_json::Error),
}

impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> ConfigError {
        ConfigError::Io(e)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> ConfigError {
        use serde_json::error::Category;
        match e.classify() {
            Category::Io => ConfigError::Io(e.into()),
            _ => ConfigError::SerdeError(e),
        }
    }
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            ConfigError::Io(e) => write!(f, "io error: {}", e),
            ConfigError::SerdeError(e) => write!(f, "decoding error: {}", e),
        }
    }
}

impl RawConfig {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let file = File::open(path.as_ref())?;
        let config: Self = serde_json::from_reader(file)?;
        config.validate()?;
        Ok(config)
    }

    #[allow(dead_code)] // Useful for tests.
    pub(self) fn from_str(s: &str) -> Result<Self, ConfigError> {
        let config: Self = serde_json::from_str(s)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.secret_file.is_relative() {
            eprintln!("warning: configuration key `.secret_file` is relative path. This will be resolved relative to server's CWD at runtime which is most likely not what you want.");
            // " <- Fix shitty Vim syntax highlighting
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Command, RawConfig, ConfigError};
    use std::path::Path;

    macro_rules! assert_matches {
        ( $e:expr , $pat:pat ) => {
            assert_matches!($e, $pat => ())
        };
        ( $e:expr , $pat:pat => $c:expr ) => {
            match $e {
                $pat => $c,
                ref e => panic!("assertion failed: `{:?}` does not match `{}`",
                    e, stringify!($pat))
            }
        };
    }

    macro_rules! assert_contains {
        ( $a:expr , $b:expr ) => {
            let a_string: String = $a.to_string();
            let b_string: String = $b.to_string();

            if !a_string.contains(&b_string) {
                panic!("assertion failed: expected {:?} to contain {:?}", a_string, b_string)
            }
        };
    }

    #[test]
    fn load_valid_raw_config() {
        let config_json = r#"
            {
                "secret_file": "/path/to/secret.txt",

                "commands": [
                    {
                        "event": "ping",
                        "command": "/usr/bin/handle-ping",
                        "args": []
                    }
                ]
            }
        "#;
        let parsed_config = RawConfig::from_str(config_json).expect("valid config");
        let expected_config = RawConfig {
            secret_file: Path::new("/path/to/secret.txt").to_path_buf(),
            commands: vec![
                Command {
                    event: "ping".to_string(),
                    command: "/usr/bin/handle-ping".to_string(),
                    args: vec![],
                },
            ],
        };
        assert_eq!(parsed_config, expected_config);
    }

    #[test]
    fn args_are_optional() {
        let command_json = r#"
            {
                "event": "ping",
                "command": "/usr/bin/handle-ping"
            }
        "#;
        let parsed_command: Command = serde_json::from_str(command_json)
            .expect("valid configuration");
        let expected_command = Command {
            event: "ping".to_string(),
            command: "/usr/bin/handle-ping".to_string(),
            args: vec![],
        };
        assert_eq!(expected_command, parsed_command);
    }

    #[test]
    fn invalid_json_gives_error() {
        // This JSON has a trailing comma, which isn't allowed.
        let config_json = r#"
            {
                "secret_file": "blah",
                "commands": [],
            }
        "#;
        let result = RawConfig::from_str(config_json);
        let err = assert_matches!(result, Err(ConfigError::SerdeError(e)) => e);
        assert_eq!(err.line(), 5);
        assert_eq!(err.column(), 13);
        assert!(err.is_syntax());
    }

    #[test]
    fn read_valid_config() {
        let parse_result = Config::from_path("examples/config.json");
        let parsed_config = assert_matches!(parse_result, Ok(c @ Config { .. }) => c);
        let expected_config = Config {
            secret: "mysecret".to_string(),
            commands: vec![
                Command {
                    event: "ping".to_string(),
                    command: "/bin/echo".to_string(),
                    args: vec![
                        "Got ping event!!".to_string()
                    ],
                },
                Command {
                    event: "push".to_string(),
                    command: "/bin/echo".to_string(),
                    args: vec![
                        "Got push event!!".to_string()
                    ],
                },
            ],
        };
        assert_eq!(parsed_config, expected_config);
    }
}
