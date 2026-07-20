//! Retained regular-file authority operations.

use super::*;

impl RegularFileAuthority {
    pub fn len(&self) -> Result<u64> {
        let metadata = self.handle.metadata()?;
        validate_real_file(Path::new("<retained file>"), &metadata)?;
        Ok(metadata.len())
    }

    /// Whether the retained file currently has zero bytes. Shares the same
    /// identity-checked metadata read as [`Self::len`].
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    pub fn read_bounded(&self, max_bytes: u64) -> Result<Vec<u8>> {
        let metadata = self.handle.metadata()?;
        validate_real_file(Path::new("<retained file>"), &metadata)?;
        let held = handle_directory_identity(&self.handle, &metadata)?;
        if held != self.identity {
            return Err(file_identity_changed(
                self.display_path(),
                "after file authority was retained",
            ));
        }
        if metadata.len() > max_bytes {
            return Err(SandboxError::PathDenied(format!(
                "authority file exceeds {max_bytes} bytes"
            )));
        }
        let mut handle = self.handle.try_clone()?;
        handle.rewind()?;
        let mut value = Vec::with_capacity(metadata.len() as usize);
        handle
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut value)?;
        if value.len() as u64 > max_bytes {
            return Err(SandboxError::PathDenied(format!(
                "authority file exceeds {max_bytes} bytes"
            )));
        }
        Ok(value)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let before_metadata = std::fs::symlink_metadata(path)?;
        validate_real_file(path, &before_metadata)?;
        let before = path_file_identity(path, &before_metadata)?;
        let handle = open_regular_file(path)?;
        let handle_metadata = handle.metadata()?;
        validate_real_file(path, &handle_metadata)?;
        let held = handle_directory_identity(&handle, &handle_metadata)?;
        let after_metadata = std::fs::symlink_metadata(path)?;
        validate_real_file(path, &after_metadata)?;
        let after = path_file_identity(path, &after_metadata)?;
        if before != held || held != after {
            return Err(file_identity_changed(
                path,
                "while file authority was acquired",
            ));
        }
        Ok(Self {
            handle,
            identity: held,
            display_path: path.to_path_buf(),
        })
    }

    pub fn validate_path(&self, path: &Path) -> Result<()> {
        let before_metadata = std::fs::symlink_metadata(path)?;
        validate_real_file(path, &before_metadata)?;
        let before = path_file_identity(path, &before_metadata)?;
        let held_metadata = self.handle.metadata()?;
        validate_real_file(path, &held_metadata)?;
        let held = handle_directory_identity(&self.handle, &held_metadata)?;
        let after_metadata = std::fs::symlink_metadata(path)?;
        validate_real_file(path, &after_metadata)?;
        let after = path_file_identity(path, &after_metadata)?;
        if held != self.identity || before != held || held != after {
            return Err(file_identity_changed(
                path,
                "after file authority was retained",
            ));
        }
        Ok(())
    }

    pub fn display_path(&self) -> &Path {
        &self.display_path
    }

    pub fn sync(&self) -> Result<()> {
        self.handle.sync_all()?;
        Ok(())
    }

    /// Read a small authority-bearing file without permitting an oversized
    /// regular file to consume unbounded memory.
    pub fn read_bounded_to_string(&self, max_bytes: u64) -> Result<String> {
        String::from_utf8(self.read_bounded(max_bytes)?).map_err(|error| {
            SandboxError::PathDenied(format!("authority file is not UTF-8: {error}"))
        })
    }

    #[cfg(windows)]
    pub(super) fn rename_into(
        &self,
        target_parent: &DirectoryAuthority,
        name: &str,
        replace: bool,
    ) -> Result<()> {
        validate_child_name(name)?;
        windows::rename_file_into(self, target_parent, name, replace)
    }
}
