use std::{
    env, fs,
    process::{Command, ExitCode},
};

use genoa_settings::idle::{IdleProfile, IdleSettings, runtime_path, write_runtime_value};

const SWAYIDLE: &str = "/usr/bin/swayidle";
const GENOA_LOCK: &str = "/usr/local/bin/genoa-lock";
const GENOA_IDLE_SUSPEND: &str = "/usr/local/bin/genoa-idle-suspend";
const BRIGHTNESSCTL: &str = "/usr/bin/brightnessctl";
const SYSTEMD_INHIBIT: &str = "/usr/bin/systemd-inhibit";
const BRIGHTNESS_STATE: &str = "idle-brightness";

#[derive(Clone, Copy)]
enum Compositor {
    Niri,
    Sway,
}

impl Compositor {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "niri" => Ok(Self::Niri),
            "sway" => Ok(Self::Sway),
            _ => Err("expected compositor to be niri or sway".to_string()),
        }
    }

    const fn display_off_command(self) -> &'static str {
        match self {
            Self::Niri => "/usr/bin/niri msg action power-off-monitors",
            Self::Sway => "/usr/bin/swaymsg 'output * dpms off'",
        }
    }

    const fn display_on_command(self) -> &'static str {
        match self {
            Self::Niri => "/usr/bin/niri msg action power-on-monitors",
            Self::Sway => "/usr/bin/swaymsg 'output * dpms on'",
        }
    }

    const fn idle_profile(self) -> IdleProfile {
        match self {
            Self::Niri => IdleProfile::Niri,
            Self::Sway => IdleProfile::Sway,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("genoa-idle: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args();
    let _program = arguments.next();
    let command = arguments.next().ok_or_else(|| {
        "usage: genoa-idle <run|dim|restore|lock|suspend|before-sleep|inhibit-suspend>".to_string()
    })?;

    match command.as_str() {
        "run" => {
            let compositor = arguments
                .next()
                .ok_or_else(|| "usage: genoa-idle run <niri|sway>".to_string())?;
            if arguments.next().is_some() {
                return Err("usage: genoa-idle run <niri|sway>".to_string());
            }
            run_swayidle(Compositor::parse(&compositor)?)
        }
        "dim" if arguments.next().is_none() => dim_display(),
        "restore" if arguments.next().is_none() => restore_display_brightness(),
        "lock" if arguments.next().is_none() => spawn_lock(&[]),
        "suspend" if arguments.next().is_none() => suspend_when_locked(),
        "before-sleep" if arguments.next().is_none() => lock_before_sleep(),
        "inhibit-suspend" => {
            let activity = arguments.next().ok_or_else(|| {
                "usage: genoa-idle inhibit-suspend <building|updating> <command> [arguments...]"
                    .to_string()
            })?;
            let command = arguments.collect::<Vec<_>>();
            inhibit_suspend(&activity, &command)
        }
        _ => Err(
            "usage: genoa-idle <run|dim|restore|lock|suspend|before-sleep|inhibit-suspend>"
                .to_string(),
        ),
    }
}

fn inhibit_suspend(activity: &str, command: &[String]) -> Result<(), String> {
    let settings = IdleSettings::load_or_default(IdleSettings::default()).0;
    let (enabled, reason) = match activity {
        "building" => (
            settings.inhibit_suspend_while_building,
            "Genoa is compiling code",
        ),
        "updating" => (
            settings.inhibit_suspend_while_updating,
            "Genoa is updating the system",
        ),
        _ => {
            return Err(
                "usage: genoa-idle inhibit-suspend <building|updating> <command> [arguments...]"
                    .to_string(),
            );
        }
    };
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| "an inhibited command is required".to_string())?;
    let mut process = if enabled {
        let mut process = Command::new(SYSTEMD_INHIBIT);
        process.args([
            "--what=sleep",
            "--mode=block",
            "--who=Genoa",
            "--why",
            reason,
            "--",
        ]);
        process.arg(program).args(arguments);
        process
    } else {
        let mut process = Command::new(program);
        process.args(arguments);
        process
    };
    let status = process
        .status()
        .map_err(|error| format!("could not start {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

fn run_swayidle(compositor: Compositor) -> Result<(), String> {
    let (settings, warning) = IdleSettings::load_or_default(compositor.idle_profile().defaults());
    if let Some(warning) = warning {
        eprintln!("genoa-idle: {warning}; using Genoa's safe defaults");
    }
    let executable = env::current_exe()
        .map_err(|error| format!("could not determine the genoa-idle path: {error}"))?;
    let executable = executable
        .to_str()
        .ok_or_else(|| "the genoa-idle path is not valid UTF-8".to_string())?;
    let arguments = swayidle_arguments(compositor, &settings, executable);
    let status = Command::new(SWAYIDLE)
        .args(arguments)
        .status()
        .map_err(|error| format!("could not start swayidle: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("swayidle exited with {status}"))
    }
}

fn swayidle_arguments(
    compositor: Compositor,
    settings: &IdleSettings,
    executable: &str,
) -> Vec<String> {
    let executable = shell_quote(executable);
    let command = |name: &str| format!("{executable} {name}");
    vec![
        "-w".to_string(),
        "timeout".to_string(),
        settings.dim_after_seconds.to_string(),
        command("dim"),
        "resume".to_string(),
        command("restore"),
        "timeout".to_string(),
        settings.lock_after_seconds.to_string(),
        command("lock"),
        "timeout".to_string(),
        settings.display_off_after_seconds.to_string(),
        compositor.display_off_command().to_string(),
        "resume".to_string(),
        compositor.display_on_command().to_string(),
        "timeout".to_string(),
        settings.suspend_after_seconds.to_string(),
        command("suspend"),
        "before-sleep".to_string(),
        command("before-sleep"),
    ]
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn dim_display() -> Result<(), String> {
    let output = Command::new(BRIGHTNESSCTL)
        .arg("get")
        .output()
        .map_err(|error| format!("could not read the current brightness: {error}"))?;
    if !output.status.success() {
        return Err(format!("brightnessctl get exited with {}", output.status));
    }
    let brightness = String::from_utf8(output.stdout)
        .map_err(|_| "brightnessctl returned non-UTF-8 output".to_string())?;
    let brightness = brightness.trim();
    brightness
        .parse::<u64>()
        .map_err(|_| "brightnessctl returned an invalid brightness value".to_string())?;
    write_runtime_value(BRIGHTNESS_STATE, brightness)?;
    run_status(
        Command::new(BRIGHTNESSCTL).args(["set", "0%"]),
        "could not dim the display",
    )
}

fn restore_display_brightness() -> Result<(), String> {
    let path = runtime_path(BRIGHTNESS_STATE)?;
    let brightness = fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let brightness = brightness.trim();
    brightness
        .parse::<u64>()
        .map_err(|_| "the saved brightness value is invalid".to_string())?;
    run_status(
        Command::new(BRIGHTNESSCTL).args(["set", brightness]),
        "could not restore display brightness",
    )?;
    fs::remove_file(path).map_err(|error| format!("could not remove the saved brightness: {error}"))
}

fn spawn_lock(arguments: &[&str]) -> Result<(), String> {
    Command::new(GENOA_LOCK)
        .args(arguments)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start Genoa Lock: {error}"))
}

fn lock_before_sleep() -> Result<(), String> {
    let _ = spawn_lock(&[]);
    wait_until_locked()
}

fn suspend_when_locked() -> Result<(), String> {
    let _ = spawn_lock(&[]);
    wait_until_locked()?;
    run_status(
        &mut Command::new(GENOA_IDLE_SUSPEND),
        "could not run Genoa's resume-safe suspend helper",
    )
}

fn wait_until_locked() -> Result<(), String> {
    run_status(
        Command::new(GENOA_LOCK).arg("--wait-until-locked"),
        "Genoa Lock did not become ready",
    )
}

fn run_status(command: &mut Command, context: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("{context}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{context}: {status}"))
    }
}

#[cfg(test)]
mod tests {
    use genoa_settings::idle::IdleSettings;

    use super::{Compositor, shell_quote, swayidle_arguments};

    #[test]
    fn swayidle_commands_use_the_validated_settings() {
        let settings = IdleSettings::default();
        let arguments = swayidle_arguments(
            Compositor::Sway,
            &settings,
            "/home/test/.cargo/bin/genoa-idle",
        );

        assert!(arguments.windows(2).any(|pair| pair == ["timeout", "180"]));
        assert!(arguments.windows(2).any(|pair| pair == ["timeout", "1200"]));
        assert!(
            arguments
                .iter()
                .any(|argument| argument.contains("dpms off"))
        );
    }

    #[test]
    fn shell_quotes_paths_without_interpreting_single_quotes() {
        assert_eq!(
            shell_quote("/home/a'b/bin/genoa-idle"),
            "'/home/a'\"'\"'b/bin/genoa-idle'"
        );
    }
}
