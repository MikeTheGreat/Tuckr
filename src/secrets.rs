//! Manages encrypted files
//!
//! Encrypts files into dotfiles/Secrets using the chacha20poly1305 algorithm

use crate::dotfiles::{self, Dotfile, ReturnCode};
use crate::fileops::DirWalk;
use chacha20poly1305::{aead::Aead, AeadCore, KeyInit, XChaCha20Poly1305};
use owo_colors::OwoColorize;
use rand::rngs;
use rust_i18n::t;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct SecretsHandler {
    dotfiles_dir: PathBuf,
    key: chacha20poly1305::Key,
    nonce: chacha20poly1305::XNonce,
}

impl SecretsHandler {
    fn try_new(profile: Option<String>) -> Result<Self, ExitCode> {
        let dotfiles_dir = match dotfiles::get_dotfiles_path(profile) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("{e}");
                return Err(ReturnCode::CouldntFindDotfiles.into());
            }
        };

        // makes a hash of the password so that it can fit on the 256 bit buffer used by the
        // algorithm
        let input_key = rpassword::prompt_password(format!("{}: ", t!("info.password"))).unwrap();
        let input_hash = Sha256::digest(input_key);

        Ok(SecretsHandler {
            dotfiles_dir,
            key: input_hash,
            nonce: XChaCha20Poly1305::generate_nonce(&mut rngs::OsRng),
        })
    }

    /// takes a path to a file and returns its encrypted content
    fn encrypt(&self, dotfile: &str) -> Result<Vec<u8>, ExitCode> {
        let cipher = XChaCha20Poly1305::new(&self.key);
        let Ok(dotfile) = fs::read(dotfile) else {
            eprintln!("{}", t!("errors.x_doesnt_exist", x = dotfile).red());
            return Err(ReturnCode::NoSuchFileOrDir.into());
        };

        match cipher.encrypt(&self.nonce, dotfile.as_slice()) {
            Ok(f) => Ok(f),
            Err(e) => {
                eprintln!("{}", e.red());
                Err(ReturnCode::EncryptionFailed.into())
            }
        }
    }

    /// takes a path to a file and returns its decrypted content
    fn decrypt(&self, dotfile: &str) -> Result<Vec<u8>, ExitCode> {
        let cipher = XChaCha20Poly1305::new(&self.key);
        let dotfile = fs::read(dotfile).expect("Couldn't read dotfile");

        // extracts the nonce from the first 24 bytes in the file
        let (nonce, contents) = dotfile.split_at(24);

        match cipher.decrypt(nonce.into(), contents) {
            Ok(f) => Ok(f),
            Err(_) => {
                eprintln!("{}", t!("errors.wrong_password").red());
                Err(ReturnCode::DecryptionFailed.into())
            }
        }
    }
}

/// Encrypts secrets
pub fn encrypt_cmd(
    profile: Option<String>,
    group: &str,
    dotfiles: &[String],
) -> Result<(), ExitCode> {
    let handler = SecretsHandler::try_new(profile)?;

    let dest_dir = handler.dotfiles_dir.join("Secrets").join(group);
    if !dest_dir.exists() {
        fs::create_dir_all(&dest_dir).unwrap();
    }

    // canonicalizing the home_dir so that it can work with
    // windows' NT UNC paths (the paths used by fs::canonicalize on windows)
    let home_dir = dirs::home_dir().unwrap().canonicalize().unwrap();

    for dotfile in dotfiles {
        let target_file = Path::new(dotfile).canonicalize().unwrap();
        let target_file = target_file.strip_prefix(&home_dir).unwrap();

        let dir_path = {
            let mut tf = target_file.to_path_buf();
            tf.pop();
            tf
        };

        let mut encrypted = handler.encrypt(dotfile)?;
        let mut encrypted_file = handler.nonce.to_vec();

        // makes sure all parent directories of the dotfile are created
        fs::create_dir_all(dest_dir.join(dir_path)).unwrap();

        // appends a 24 byte nonce to the beginning of the file
        encrypted_file.append(&mut encrypted);
        fs::write(dest_dir.join(target_file), encrypted_file).unwrap();
    }

    Ok(())
}

/// Decrypts secrets
pub fn decrypt_cmd(
    profile: Option<String>,
    groups: &[String],
    exclude: &[String],
) -> Result<(), ExitCode> {
    let handler = SecretsHandler::try_new(profile.clone())?;

    if let Some(invalid_groups) =
        dotfiles::check_invalid_groups(profile, dotfiles::DotfileType::Secrets, groups)
    {
        for group in invalid_groups {
            eprintln!("{}", t!("errors.no_group", group = group).red());
        }
        return Err(ReturnCode::DecryptionFailed.into());
    }

    let dest_dir = std::env::current_dir().unwrap();

    let decrypt_group = |group: Dotfile| -> Result<(), ExitCode> {
        if exclude.contains(&group.group_name) || !group.is_valid_target() {
            return Ok(());
        }

        let group_dir = handler.dotfiles_dir.join("Secrets").join(&group.group_path);
        for secret in DirWalk::new(group_dir) {
            if secret.is_dir() {
                continue;
            }

            let decrypted = handler.decrypt(secret.to_str().unwrap())?;

            fs::write(dest_dir.join(secret.file_name().unwrap()), decrypted).unwrap();
        }

        Ok(())
    };

    if groups.contains(&"*".to_string()) {
        let groups_dir = handler.dotfiles_dir.join("Secrets");
        for group in fs::read_dir(groups_dir).unwrap() {
            let Ok(group) = Dotfile::try_from(group.unwrap().path()) else {
                eprintln!("{}", t!("errors.got_invalid_group").red());
                return Err(ExitCode::FAILURE);
            };
            decrypt_group(group)?;
        }

        return Ok(());
    }

    for group in groups {
        let group = handler.dotfiles_dir.join("Secrets").join(group);
        let Ok(group) = Dotfile::try_from(group) else {
            eprintln!("{}", t!("errors.got_invalid_group").red());
            return Err(ExitCode::FAILURE);
        };
        decrypt_group(group)?;
    }

    Ok(())
}
