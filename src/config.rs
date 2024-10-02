use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io;
use std::fmt::{self, Display};
use serde::Deserialize;
use std::time::Duration;

/// All the application configuration is stored in this structure.
#[derive(Deserialize, PartialEq, Clone, Debug)]
pub struct Config {
    /// Path to the file containing the GitHub secret.
    pub secret_path: PathBuf,

    /// The secret string shared with GitHub that is used to verify signed requests.
    #[serde(skip_deserializing)]
    pub secret: String,

    /// Event-command pairs. Each element of this array should be matched (and optionally executed)
    /// against the commands in gaide.
    pub commands: Vec<Command>,

    /// The maximum time the server should spend sitting idle waiting for a connection before
    /// shutting itself down.
    ///
    /// This is pretty relevant as webhook event are relatively rare. Shutting down and waiting for
    /// socket (re)activation spares a few ressources.
    #[serde(default)]
    #[serde(with = "humantime_serde")]
    pub max_idle_time: Option<Duration>,
}

impl Config {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let file = File::open(path.as_ref()).map_err(ConfigError::IoReadingConfig)?;
        let mut config: Config = serde_json::from_reader(file)?;

        if config.secret_path.is_relative() {
            eprintln!("warning: `secret_path` in configuration is a relative path.\
                       This will be resolved relative to the server's CWD at runtime,\
                       which is most likely not what you want!");
        }
        config.secret = fs::read_to_string(&config.secret_path)
            .map_err(ConfigError::IoReadingSecret)?;

        Ok(config)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> ConfigError {
        use serde_json::error::Category;
        match e.classify() {
            Category::Io => ConfigError::IoReadingConfig(e.into()),
            _ => ConfigError::SerdeError(e),
        }
    }
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

/// Errors that can occur when reading configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// An IO error occured while reading the configuration, such as failing to read the file.
    IoReadingConfig(io::Error),
    /// An IO error occured while reading the secret file linked via `secret_path`.
    IoReadingSecret(io::Error),
    /// Decoding the file failed, e.g. if JSON is missing comma.
    SerdeError(serde_json::Error),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            ConfigError::IoReadingConfig(e) => write!(f, "io error while reading configuration file: {}", e),
            ConfigError::IoReadingSecret(e) => write!(f, "io error while reading secret file: {}", e),
            ConfigError::SerdeError(e) => write!(f, "decoding error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Command, ConfigError};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    macro_rules! assert_matches {
        ( $e:expr , $pat:pat ) => {
            assert_matches!($e, $pat => ())
        };
        ( $e:expr , $pat:pat => $c:expr ) => {
            match $e {
                $pat => $c,
                ref e => panic!("assertion failed: `{:?}` does not match `{}`", e, stringify!($pat))
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
    fn deserialize_valid_config() {
        let config_json = r#"
            {
                "secret_path": "/path/to/secret.txt",

                "max_idle_time": "10min",

                "commands": [
                    {
                        "event": "ping",
                        "command": "/usr/bin/handle-ping",
                        "args": []
                    }
                ]
            }
        "#;
        let parsed_config = serde_json::from_str::<Config>(config_json).expect("valid config");
        let expected_config = Config {
            secret_path: Path::new("/path/to/secret.txt").to_path_buf(),
            secret: "".to_string(), // We didn't ask it to read file
            max_idle_time: Some(Duration::from_secs(600)),
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
    fn deserialize_command_without_optional_args() {
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
    fn deserialize_invalid_json_gives_error() {
        // This JSON has a trailing comma, which isn't allowed.
        let config_json = r#"
            {
                "secret_path": "blah",
                "commands": [],
            }
        "#;
        // This way we also test the error wrapping code in our implementation of `std::convert::from::From`.
        let result: Result<Config, ConfigError> = serde_json::from_str::<Config>(config_json).map_err(|e| e.into());
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
            secret_path: PathBuf::from("./examples/secret.txt"),
            secret: "mysecret".to_string(),
            max_idle_time: Some(Duration::from_secs(60 * 60)),
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
