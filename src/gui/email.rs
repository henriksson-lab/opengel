use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::SmtpTransportBuilder;
use lettre::{Message, SmtpTransport, Transport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSettings {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub from_address: String,
    #[serde(default)]
    pub security: EmailSecurity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EmailSecurity {
    StartTls,
    Tls,
    None,
}

impl Default for EmailSettings {
    fn default() -> Self {
        Self {
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_address: String::new(),
            security: EmailSecurity::StartTls,
        }
    }
}

impl Default for EmailSecurity {
    fn default() -> Self {
        Self::StartTls
    }
}

impl EmailSecurity {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Tls,
            2 => Self::None,
            _ => Self::StartTls,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Self::StartTls => 0,
            Self::Tls => 1,
            Self::None => 2,
        }
    }
}

pub fn load_settings() -> EmailSettings {
    let Ok(path) = settings_path() else {
        return EmailSettings::default();
    };
    let Ok(bytes) = fs::read(path) else {
        return EmailSettings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save_settings(settings: &EmailSettings) -> Result<PathBuf> {
    let path = settings_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("email settings path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(settings)?;
    fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn send_data_file(
    settings: &EmailSettings,
    to_address: &str,
    attachment_name: &str,
    data: Vec<u8>,
) -> Result<()> {
    settings.validate()?;
    let from: Mailbox = settings
        .from_address
        .parse()
        .with_context(|| format!("invalid sender address `{}`", settings.from_address))?;
    let to: Mailbox = to_address
        .trim()
        .parse()
        .with_context(|| format!("invalid recipient address `{}`", to_address.trim()))?;
    let content_type = ContentType::parse("application/zip")?;
    let attachment = Attachment::new(attachment_name.to_string()).body(data, content_type);
    let body = SinglePart::plain("OpenGel data file attached.".to_string());
    let email = Message::builder()
        .from(from)
        .to(to)
        .subject("OpenGel data file")
        .multipart(MultiPart::mixed().singlepart(body).singlepart(attachment))?;

    let mailer = match settings.security {
        EmailSecurity::StartTls => apply_credentials(
            SmtpTransport::starttls_relay(&settings.smtp_host)?.port(settings.smtp_port),
            settings,
        )
        .build(),
        EmailSecurity::Tls => apply_credentials(
            SmtpTransport::relay(&settings.smtp_host)?.port(settings.smtp_port),
            settings,
        )
        .build(),
        EmailSecurity::None => apply_credentials(
            SmtpTransport::builder_dangerous(&settings.smtp_host)
                .port(settings.smtp_port)
                .tls(Tls::None),
            settings,
        )
        .build(),
    };
    mailer.send(&email)?;
    Ok(())
}

impl EmailSettings {
    fn validate(&self) -> Result<()> {
        if self.smtp_host.trim().is_empty() {
            return Err(anyhow!("SMTP host is not configured"));
        }
        if self.from_address.trim().is_empty() {
            return Err(anyhow!("sender address is not configured"));
        }
        if self.smtp_username.trim().is_empty() && !self.smtp_password.is_empty() {
            return Err(anyhow!(
                "SMTP username is required when an SMTP password is configured"
            ));
        }
        Ok(())
    }
}

fn apply_credentials(
    builder: SmtpTransportBuilder,
    settings: &EmailSettings,
) -> SmtpTransportBuilder {
    if settings.smtp_username.trim().is_empty() {
        builder
    } else {
        builder.credentials(Credentials::new(
            settings.smtp_username.clone(),
            settings.smtp_password.clone(),
        ))
    }
}

fn settings_path() -> Result<PathBuf> {
    let base =
        config_base_dir().ok_or_else(|| anyhow!("could not locate user config directory"))?;
    Ok(base.join("OpenGel").join("email-settings.json"))
}

#[cfg(target_os = "windows")]
fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library").join("Application Support"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}
