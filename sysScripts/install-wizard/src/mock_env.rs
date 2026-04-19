use crate::traits::CmdExecutor;
use std::cell::RefCell;

#[allow(dead_code)]
pub struct MockEnv {
    pub env_vars: std::collections::HashMap<String, String>,
    pub cmd_log: RefCell<Vec<(String, Vec<String>)>>,
    pub mock_files: std::collections::HashMap<String, String>,
}

impl CmdExecutor for MockEnv {
    fn run_cmd(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error> {
        self.cmd_log.borrow_mut().push((
            cmd.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));
        Ok(())
    }
    fn run_cmd_ignore_err(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error> {
        self.cmd_log.borrow_mut().push((
            cmd.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));
        Ok(())
    }
    fn read_file_to_string(&self, path: &str) -> Result<String, std::io::Error> {
        if let Some(content) = self.mock_files.get(path) {
            Ok(content.clone())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("File '{}' not found in mock environment", path),
            ))
        }
    }
    fn get_env_var(&self, key: &str) -> Option<String> {
        self.env_vars.get(key).cloned()
    }
    fn path_exists(&self, path: &std::path::Path) -> bool {
        path.to_str()
            .map(|s| self.mock_files.contains_key(s))
            .unwrap_or(false)
    }
}
