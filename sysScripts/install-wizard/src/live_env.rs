use crate::traits::CmdExecutor;
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

pub struct LiveEnv;

impl CmdExecutor for LiveEnv {
    fn run_cmd(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error> {
        let status = Command::new(cmd).args(args).status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "Command '{}' with args {:?} failed",
                cmd, args
            )));
        }
        Ok(())
    }
    fn run_cmd_ignore_err(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error> {
        let _ = Command::new(cmd).args(args).status();
        Ok(())
    }
    fn read_file_to_string(&self, path: &std::path::Path) -> Result<String, std::io::Error> {
        std::fs::read_to_string(path)
    }
    fn get_env_var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
    fn path_exists(&self, path: &std::path::Path) -> bool {
        path.exists()
    }
    fn write_string_to_file(&self, path: &str, content: &str) -> Result<(), std::io::Error> {
        std::fs::write(path, content)
    }
    fn create_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(path)
    }
    fn install_string_to_root_file(
        &self,
        dest_path: &std::path::Path,
        content: &str,
        mode: &str,
    ) -> Result<bool, std::io::Error> {
        if let Ok(existing_content) = self.read_file_to_string(dest_path)
            && existing_content == content
        {
            return Ok(false);
        }
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(content.as_bytes())?;
        let temp_path = temp_file.path();
        self.run_cmd(
            "sudo",
            &[
                "install",
                "-m",
                mode,
                "-o",
                "root",
                "-g",
                "root",
                temp_path.to_str().unwrap(),
                dest_path.to_str().unwrap(),
            ],
        )?;
        Ok(true)
    }
    fn create_root_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        self.run_cmd("sudo", &["mkdir", "-p", path.to_str().unwrap()])?;
        self.run_cmd("sudo", &["chown", "root:root", path.to_str().unwrap()])?;
        Ok(())
    }
    fn list_dir_file_names(&self, path: &std::path::Path) -> Result<Vec<String>, std::io::Error> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }
    fn is_symlink(&self, path: &std::path::Path) -> bool {
        path.is_symlink()
    }
}
