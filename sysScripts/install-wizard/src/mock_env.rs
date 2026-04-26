use crate::traits::CmdExecutor;
use std::cell::RefCell;

#[allow(dead_code)]
#[derive(Default)]
pub struct MockEnv {
    pub env_vars: std::collections::HashMap<String, String>,
    pub cmd_log: RefCell<Vec<(String, Vec<String>)>>,
    pub mock_files: RefCell<std::collections::HashMap<String, String>>,
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
        if let Some(content) = self.mock_files.borrow().get(path) {
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
            .map(|s| self.mock_files.borrow().contains_key(s))
            .unwrap_or(false)
    }
    fn create_dir_all(&self, _path: &std::path::Path) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn write_string_to_file(&self, path: &str, content: &str) -> Result<(), std::io::Error> {
        self.mock_files
            .borrow_mut()
            .insert(path.to_string(), content.to_string());
        Ok(())
    }
    fn install_string_to_root_file(
        &self,
        dest_path: &str,
        content: &str,
        mode: &str,
    ) -> Result<(), std::io::Error> {
        self.mock_files
            .borrow_mut()
            .insert(dest_path.to_string(), content.to_string());
        self.cmd_log.borrow_mut().push((
            "sudo".to_string(),
            vec![
                "install".to_string(),
                "-m".to_string(),
                mode.to_string(),
                "-o".to_string(),
                "root".to_string(),
                "-g".to_string(),
                "root".to_string(),
                "/tmp/mock_file".to_string(),
                dest_path.to_string(),
            ],
        ));
        Ok(())
    }
    fn create_root_dir_all(&self, _path: &std::path::Path) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn list_dir_file_names(&self, path: &std::path::Path) -> Result<Vec<String>, std::io::Error> {
        let path_str = path
            .to_str()
            .ok_or_else(|| std::io::Error::other("Invalid directory path"))?;
        let prefix = format!("{}/", path_str.trim_end_matches('/'));
        let files = self.mock_files.borrow();
        let mut names = Vec::new();

        for key in files.keys() {
            if key.starts_with(&prefix)
                && let Some(name) = std::path::Path::new(key)
                    .file_name()
                    .and_then(|n| n.to_str())
            {
                names.push(name.to_string());
            }
        }

        names.sort();
        names.dedup();
        Ok(names)
    }
}
