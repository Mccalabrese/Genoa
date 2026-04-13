pub trait CmdExecutor {
    fn run_cmd(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error>;
    fn run_cmd_ignore_err(&self, cmd: &str, args: &[&str]) -> Result<(), std::io::Error>;
    fn read_file_to_string(&self, path: &str) -> Result<String, std::io::Error>;
}
