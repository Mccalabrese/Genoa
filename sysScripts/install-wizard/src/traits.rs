pub trait CmdExecutor {
    fn run_cmd(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error>;
    fn run_cmd_ignore_err(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error>;
    fn run_cmd_silently_ignore_err(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error>;
    fn run_cmd_in_dir(
        &self,
        dir: &std::path::Path,
        cmd: &str,
        args: &[&str],
    ) -> Result<(), std::io::Error>;
    fn command_exists(&self, cmd: &str) -> bool;
    fn is_package_installed(&self, package: &str) -> bool;
    fn read_file_to_string(&self, path: &std::path::Path) -> Result<String, std::io::Error>;
    fn get_env_var(&self, key: &str) -> Option<String>;
    fn path_exists(&self, path: &std::path::Path) -> bool;
    fn path_is_dir(&self, path: &std::path::Path) -> bool;
    fn write_string_to_file(&self, path: &str, content: &str) -> Result<(), std::io::Error>;
    fn create_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error>;
    fn remove_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error>;
    fn rename_path(
        &self,
        from: &std::path::Path,
        to: &std::path::Path,
    ) -> Result<(), std::io::Error>;
    fn install_string_to_root_file(
        &self,
        dest_path: &std::path::Path,
        content: &str,
        mode: &str,
    ) -> Result<bool, std::io::Error>;
    fn create_root_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error>;
    fn list_dir_file_names(&self, path: &std::path::Path) -> Result<Vec<String>, std::io::Error>;
    fn is_symlink(&self, path: &std::path::Path) -> bool;
}
