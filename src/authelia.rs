//! Authelia user database synchronization
//!
//! This module handles synchronization of user credentials from HaLOS branding
//! to Authelia's file-based user database.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2, Params,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::{AdapterError, Result};

/// Authelia user database structure
#[derive(Debug, Serialize, Deserialize)]
pub struct UsersDatabase {
    pub users: HashMap<String, User>,
}

/// Individual user entry in Authelia
#[derive(Debug, Serialize, Deserialize)]
pub struct User {
    pub displayname: String,
    pub password: String,
    pub email: String,
    #[serde(default)]
    pub groups: Vec<String>,
}

impl UsersDatabase {
    /// Load users database from file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            // Return empty database if file doesn't exist
            return Ok(Self {
                users: HashMap::new(),
            });
        }

        let contents = fs::read_to_string(path)?;
        let db: UsersDatabase = serde_yaml::from_str(&contents).map_err(|e| {
            AdapterError::Config(format!("Failed to parse Authelia users database: {}", e))
        })?;

        Ok(db)
    }

    /// Save users database to file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let contents = serde_yaml::to_string(self).map_err(|e| {
            AdapterError::State(format!(
                "Failed to serialize Authelia users database: {}",
                e
            ))
        })?;

        // Add header comment
        let output = format!(
            "# Authelia Users Database\n\
             # This file is managed by homarr-container-adapter\n\
             # Manual edits may be overwritten\n\n{}",
            contents
        );

        fs::write(path, output)?;

        Ok(())
    }

    /// Add or update a user
    pub fn upsert_user(&mut self, username: &str, user: User) {
        self.users.insert(username.to_string(), user);
    }
}

/// Hash a password using argon2id with Authelia-compatible parameters
///
/// Authelia's default parameters:
/// - Memory: 65536 KB (64 MB)
/// - Iterations: 3
/// - Parallelism: 4
pub fn hash_password(password: &str) -> Result<String> {
    // Authelia's default argon2id parameters
    let params = Params::new(65536, 3, 4, None)
        .map_err(|e| AdapterError::Config(format!("Failed to create argon2 params: {}", e)))?;

    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let salt = SaltString::generate(&mut OsRng);

    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AdapterError::Config(format!("Failed to hash password: {}", e)))?;

    Ok(hash.to_string())
}

/// Sync credentials to Authelia user database
///
/// This function creates or updates a user in Authelia's users_database.yml
/// with credentials from the HaLOS branding configuration.
pub fn sync_credentials<P: AsRef<Path>>(
    db_path: P,
    username: &str,
    password: &str,
    email: Option<&str>,
) -> Result<()> {
    let db_path = db_path.as_ref();

    tracing::info!("Syncing credentials to Authelia: {}", db_path.display());

    // Load existing database or create new
    let mut db = UsersDatabase::load(db_path)?;

    // Hash the password
    let password_hash = hash_password(password)?;

    // Create user entry
    let user = User {
        displayname: username.to_string(),
        password: password_hash,
        email: email
            .unwrap_or(&format!("{}@halos.local", username))
            .to_string(),
        groups: vec!["admins".to_string()],
    };

    // Add/update user
    db.upsert_user(username, user);

    // Save database
    db.save(db_path)?;

    tracing::info!(
        "Authelia credentials synced successfully for user '{}'",
        username
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_hash_password_format() {
        let hash = hash_password("test_password").unwrap();
        // Should start with argon2id identifier
        assert!(hash.starts_with("$argon2id$"));
        // Should contain version
        assert!(hash.contains("v=19"));
    }

    #[test]
    fn test_users_database_load_nonexistent() {
        let result = UsersDatabase::load("/nonexistent/path/users.yml");
        assert!(result.is_ok());
        let db = result.unwrap();
        assert!(db.users.is_empty());
    }

    #[test]
    fn test_users_database_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("users_database.yml");

        let mut db = UsersDatabase {
            users: HashMap::new(),
        };

        db.upsert_user(
            "admin",
            User {
                displayname: "Admin User".to_string(),
                password: "$argon2id$test".to_string(),
                email: "admin@halos.local".to_string(),
                groups: vec!["admins".to_string()],
            },
        );

        // Save
        db.save(&db_path).unwrap();
        assert!(db_path.exists());

        // Load back
        let loaded = UsersDatabase::load(&db_path).unwrap();
        assert_eq!(loaded.users.len(), 1);
        assert!(loaded.users.contains_key("admin"));
        let admin = loaded.users.get("admin").unwrap();
        assert_eq!(admin.displayname, "Admin User");
        assert_eq!(admin.email, "admin@halos.local");
    }

    #[test]
    fn test_sync_credentials() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("users_database.yml");

        // Sync credentials
        sync_credentials(&db_path, "testuser", "testpass", Some("test@example.com")).unwrap();

        // Verify
        let db = UsersDatabase::load(&db_path).unwrap();
        assert_eq!(db.users.len(), 1);
        let user = db.users.get("testuser").unwrap();
        assert_eq!(user.displayname, "testuser");
        assert_eq!(user.email, "test@example.com");
        assert!(user.password.starts_with("$argon2id$"));
        assert!(user.groups.contains(&"admins".to_string()));
    }

    #[test]
    fn test_sync_credentials_default_email() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("users_database.yml");

        // Sync without email
        sync_credentials(&db_path, "admin", "password", None).unwrap();

        // Verify default email
        let db = UsersDatabase::load(&db_path).unwrap();
        let user = db.users.get("admin").unwrap();
        assert_eq!(user.email, "admin@halos.local");
    }
}
