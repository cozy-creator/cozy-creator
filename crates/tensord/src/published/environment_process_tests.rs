use super::*;
use platform::process::{Birth, Exact};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::Child,
};

const FIXTURE: &str = "published::environment::process_tests::builder_fixture";

fn fixture_command(role: &str, socket: &Path, namespace: &str, directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", FIXTURE, "--nocapture"])
        .env("CL853_ROLE", role)
        .env("CL853_SOCKET", socket)
        .env("CL853_NAMESPACE", namespace)
        .env("CL853_DIRECTORY", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

fn report(role: &str, socket: &Path) -> UnixStream {
    let mut connection = UnixStream::connect(socket).unwrap();
    writeln!(connection, "{role} {}", std::process::id()).unwrap();
    connection
}

/// Re-executed fixture roles use real process groups, inherited stderr and socket barriers.
/// The writer leads a separate group, so ending only the builder's group cannot pass.
#[test]
fn builder_fixture() {
    let Ok(role) = std::env::var("CL853_ROLE") else {
        return;
    };
    let socket = PathBuf::from(std::env::var_os("CL853_SOCKET").unwrap());
    let directory = PathBuf::from(std::env::var_os("CL853_DIRECTORY").unwrap());
    let namespace = std::env::var("CL853_NAMESPACE").unwrap();
    if role == "writer" {
        fs::write(
            directory.join("writer"),
            b"old builder owns this environment",
        )
        .unwrap();
    }
    let mut connection = report(&role, &socket);
    match role.as_str() {
        "owner" => {
            let stop = crate::stop::Stop::default();
            if std::env::var_os("CL853_CANCEL").is_some() {
                let (stop, mut reader) = (stop.clone(), connection.try_clone().unwrap());
                std::thread::spawn(move || {
                    let mut byte = [0];
                    if reader.read_exact(&mut byte).is_ok() && byte == *b"c" {
                        stop.cancel();
                    }
                });
            }
            let mut command = fixture_command("builder", &socket, &namespace, &directory);
            command.stderr(Stdio::piped());
            let (ended, _) = run_builder(&mut command, &namespace, Some(&stop), &|_| {}).unwrap();
            writeln!(
                connection,
                "{}",
                match ended {
                    Supervised::Exited(status) if status.success() => "exited",
                    Supervised::Canceled => "canceled",
                    other => panic!("unexpected helper exit: {other:?}"),
                }
            )
            .unwrap();
        }
        "builder" => {
            let mut command = fixture_command("writer", &socket, &namespace, &directory);
            platform::process::own_group(&mut command);
            command.stderr(Stdio::inherit()).spawn().unwrap();
            let mut byte = [0];
            let _ = connection.read_exact(&mut byte);
            // Deliberately orphan the separate-group writer, just as a stopped builder does.
            std::process::exit(0);
        }
        "writer" => {
            let mut byte = [0];
            let _ = connection.read_exact(&mut byte);
        }
        _ => panic!("unknown fixture role {role}"),
    }
}

struct Building {
    owner: Child,
    peers: BTreeMap<String, (Birth, BufReader<UnixStream>)>,
    _socket: tempfile::TempDir,
}

impl Building {
    fn start(namespace: &str, directory: &Path, cancel: bool) -> Self {
        platform::process::become_subreaper().unwrap();
        let socket = tempfile::tempdir().unwrap();
        let path = socket.path().join("ready");
        let listener = UnixListener::bind(&path).unwrap();
        let mut command = fixture_command("owner", &path, namespace, directory);
        if cancel {
            command.env("CL853_CANCEL", "1");
        } else {
            command.env_remove("CL853_CANCEL");
        }
        command.stderr(Stdio::inherit());
        let mut building = Self {
            owner: command.spawn().unwrap(),
            peers: BTreeMap::new(),
            _socket: socket,
        };
        for _ in 0..3 {
            let mut connection = BufReader::new(listener.accept().unwrap().0);
            let mut line = String::new();
            connection.read_line(&mut line).unwrap();
            let (role, pid) = line.trim().split_once(' ').unwrap();
            building.peers.insert(
                role.to_owned(),
                (Birth::of(pid.parse().unwrap()).unwrap(), connection),
            );
        }
        building
    }

    fn send(&mut self, role: &str, byte: u8) {
        self.peers
            .get_mut(role)
            .unwrap()
            .1
            .get_mut()
            .write_all(&[byte])
            .unwrap();
    }

    fn ended(&self, role: &str) -> bool {
        self.peers[role].0.ended().unwrap()
    }

    fn finished(&mut self, expected: &str) {
        let mut line = String::new();
        self.peers
            .get_mut("owner")
            .unwrap()
            .1
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), expected);
        assert!(self.owner.wait().unwrap().success());
        assert!(self.ended("builder"));
        assert!(self.ended("writer"));
    }
}

impl Drop for Building {
    fn drop(&mut self) {
        // Exact births keep a failed assertion from killing an unrelated reused PID.
        let mut processes = match self.owner.try_wait() {
            Ok(None) => platform::process::tree(self.owner.id()),
            _ => vec![],
        };
        processes.extend(
            self.peers
                .values()
                .filter_map(|(birth, _)| Exact::open(birth).ok().flatten()),
        );
        let _ = platform::process::end_tree(&processes);
        let _ = self.owner.wait();
        for (birth, _) in self.peers.values() {
            if Birth::of(birth.pid).is_ok_and(|now| now == *birth) {
                let _ = rustix::process::waitpid(
                    rustix::process::Pid::from_raw(birth.pid as i32),
                    rustix::process::WaitOptions::NOHANG,
                );
            }
        }
    }
}

#[test]
fn restart_reaps_builder_and_grandchild_after_term_and_kill() {
    for signal in [rustix::process::Signal::TERM, rustix::process::Signal::KILL] {
        let environment = tempfile::tempdir().unwrap();
        let namespace = platform::contain::namespace(environment.path());
        let unrelated = tempfile::tempdir().unwrap();
        let mut control = Building::start(
            &platform::contain::namespace(unrelated.path()),
            unrelated.path(),
            false,
        );
        let mut building = Building::start(&namespace, environment.path(), false);
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(building.owner.id() as i32).unwrap(),
            signal,
        )
        .unwrap();
        building.owner.wait().unwrap();
        assert!(!building.ended("builder"));
        assert!(!building.ended("writer"));
        assert!(platform::contain::Scope::sweep(&namespace).unwrap() >= 2);
        assert!(building.ended("builder"));
        assert!(building.ended("writer"));
        assert!(!control.ended("owner"));
        assert!(!control.ended("builder"));
        assert!(!control.ended("writer"));
        // A resumed install reuses the path only after that ownership was ended.
        fs::remove_dir_all(environment.path()).unwrap();
        fs::create_dir(environment.path()).unwrap();
        fs::write(environment.path().join("writer"), b"restarted builder").unwrap();
        control.send("builder", b'x');
        control.finished("exited");
    }
}

#[test]
fn builder_exit_ends_grandchild_before_joining_stderr() {
    let directory = tempfile::tempdir().unwrap();
    let mut building = Building::start(
        &platform::contain::namespace(directory.path()),
        directory.path(),
        false,
    );
    building.send("builder", b'x');
    building.finished("exited");
}

#[test]
fn builder_cancel_ends_grandchild_before_joining_stderr() {
    let directory = tempfile::tempdir().unwrap();
    let mut building = Building::start(
        &platform::contain::namespace(directory.path()),
        directory.path(),
        true,
    );
    building.send("owner", b'c');
    building.finished("canceled");
}
