use crate::traits::CmdExecutor;
use std::process::Command;

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
    fn read_file_to_string(&self, path: &str) -> Result<String, std::io::Error> {
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
    fn create_dir_all(&self, path: &str) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(path)
    }
}
