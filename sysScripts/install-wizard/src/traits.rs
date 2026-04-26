pub trait CmdExecutor {
    fn run_cmd(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error>;
    fn run_cmd_ignore_err(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error>;
    fn read_file_to_string(&self, path: &str) -> Result<String, std::io::Error>;
    fn get_env_var(&self, key: &str) -> Option<String>;
    fn path_exists(&self, path: &std::path::Path) -> bool;
    fn write_string_to_file(&self, path: &str, content: &str) -> Result<(), std::io::Error>;
    fn create_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error>;
    fn install_string_to_root_file(
        &self,
        dest_path: &str,
        content: &str,
        mode: &str,
    ) -> Result<(), std::io::Error>;
    fn create_root_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error>;
}
