//! Programs left running in the background (dev servers, watchers). Output
//! is kept in a bounded buffer, and every job dies with the session: dropping
//! a job kills its whole process group.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::exec::{self, Exec};
use crate::os::Group;
use crate::tools::truncate;

const MAX_JOBS: usize = 4;
/// Lines kept per job; older ones are dropped.
const MAX_LINES: usize = 200;
/// How long a start waits for early output: a URL, or a crash.
const SETTLE: Duration = Duration::from_secs(8);
/// Longest single wait on a job.
const MAX_WAIT: u64 = 300;

#[derive(Default)]
struct Log {
    lines: VecDeque<String>,
    /// Lines ever received, so readers can tell what is new.
    total: usize,
    exit: Option<String>,
    /// Local ports the job announced (`localhost:5173`): what http_local may reach.
    ports: BTreeSet<u16>,
}

struct Job {
    id: usize,
    cmd: String,
    started: Instant,
    log: Arc<Mutex<Log>>,
    /// Lines already shown to the model.
    read: usize,
    _group: Group,
}

#[derive(Default)]
pub struct Jobs {
    list: Vec<Job>,
    next: usize,
}

impl Jobs {
    pub fn running(&self) -> usize {
        self.list.iter().filter(|j| j.state().is_none()).count()
    }

    /// Ports announced by jobs still running.
    pub fn ports(&self) -> BTreeSet<u16> {
        self.list.iter().filter(|j| j.state().is_none()).flat_map(|j| j.log.lock().unwrap().ports.clone()).collect()
    }

    /// Starts `ex` and returns once it settled: its first output, or its exit.
    pub async fn start(&mut self, ex: &Exec, root: &Path) -> Result<String, String> {
        if self.running() >= MAX_JOBS {
            return Err(format!("{MAX_JOBS} background jobs already running; process stop one (ids: process status), then run again"));
        }
        let (mut child, group) = exec::spawn(ex, root)?;
        let log = Arc::new(Mutex::new(Log::default()));
        if let Some(out) = child.stdout.take() {
            tokio::spawn(pipe(out, log.clone()));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(pipe(err, log.clone()));
        }
        let status = log.clone();
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code());
            status.lock().unwrap().exit = Some(code.map_or("killed".into(), |c| format!("exit {c}")));
        });
        self.next += 1;
        let job = Job { id: self.next, cmd: ex.display(), started: Instant::now(), log, read: 0, _group: group };
        // Done once it exits, once it announced a URL, or once it printed then
        // stayed quiet for 2s (wrappers like npm print a banner long before
        // the server listens); never longer than SETTLE.
        let deadline = Instant::now() + SETTLE;
        let (mut seen, mut quiet) = (0, 0);
        while job.state().is_none() && Instant::now() < deadline && !(seen > 0 && quiet >= 20) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let log = job.log.lock().unwrap();
            if !log.ports.is_empty() {
                break;
            }
            let total = log.total;
            drop(log);
            quiet = if total == seen { quiet + 1 } else { 0 };
            seen = total;
        }
        self.list.push(job);
        Ok(exec::warned(self.output(self.next).expect("just pushed")))
    }

    /// Waits up to `secs` for the job to exit, then shows its new output:
    /// follows a long command without a tool call per poll.
    pub async fn wait(&mut self, id: usize, secs: u64) -> Result<String, String> {
        let job = self.list.iter().find(|j| j.id == id).ok_or_else(|| format!("no job {id}; see process status for the ids"))?;
        let deadline = Instant::now() + Duration::from_secs(secs.min(MAX_WAIT));
        while job.state().is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.output(id)
    }

    /// One line per job: `job 1 running 2m: npm run dev`.
    pub fn status(&self) -> String {
        if self.list.is_empty() {
            return "no background jobs".into();
        }
        self.list.iter().map(|j| format!("{}: {}", j.header(), j.cmd)).collect::<Vec<_>>().join("\n")
    }

    /// State, then the lines the model has not seen yet.
    pub fn output(&mut self, id: usize) -> Result<String, String> {
        let job = self.list.iter_mut().find(|j| j.id == id).ok_or_else(|| format!("no job {id}; see process status for the ids"))?;
        let mut out = job.header();
        let log = job.log.lock().unwrap();
        let first = log.total - log.lines.len();
        if job.read < first {
            out.push_str(&format!("\n[{} earlier lines dropped]", first - job.read));
        }
        let new: Vec<&str> = log.lines.iter().skip(job.read.saturating_sub(first)).map(String::as_str).collect();
        if !new.is_empty() {
            out.push('\n');
            out.push_str(&exec::clip(&new.join("\n")));
        }
        let total = log.total;
        drop(log);
        job.read = total;
        Ok(out)
    }

    /// Unread output of the latest job that announced `port`, running or not:
    /// why a server stopped answering, without a separate process call.
    pub fn output_on(&mut self, port: u16) -> Option<String> {
        let id = self.list.iter().rev().find(|j| j.log.lock().unwrap().ports.contains(&port))?.id;
        self.output(id).ok()
    }

    /// Kills the job and everything it spawned; returns its last output.
    pub fn stop(&mut self, id: usize) -> Result<String, String> {
        if let Some(job) = self.list.iter().find(|j| j.id == id) {
            job.log.lock().unwrap().exit.get_or_insert_with(|| "stopped".into());
        }
        let out = self.output(id)?;
        self.list.retain(|j| j.id != id);
        Ok(out)
    }
}

impl Job {
    fn state(&self) -> Option<String> {
        self.log.lock().unwrap().exit.clone()
    }

    /// `job 1 running 3s`: the id comes first so the model never guesses it.
    fn header(&self) -> String {
        let secs = self.started.elapsed().as_secs();
        let age = if secs < 120 { format!("{secs}s") } else { format!("{}m", secs / 60) };
        format!("job {} {} {age}", self.id, self.state().unwrap_or_else(|| "running".into()))
    }
}

/// Ports in local addresses a server prints: `http://localhost:5173/`.
fn ports_in(line: &str) -> Vec<u16> {
    ["localhost:", "127.0.0.1:", "[::1]:", "0.0.0.0:"]
        .iter()
        .flat_map(|host| line.match_indices(host).map(move |(i, _)| &line[i + host.len()..]))
        .filter_map(|rest| rest[..rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len())].parse().ok())
        .filter(|&p| p != 0)
        .collect()
}

async fn pipe(stream: impl AsyncRead + Unpin, log: Arc<Mutex<Log>>) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut log = log.lock().unwrap();
        log.ports.extend(ports_in(&line));
        log.lines.push_back(truncate(&line, 300).into_owned());
        log.total += 1;
        if log.lines.len() > MAX_LINES {
            log.lines.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_announced_local_ports() {
        assert_eq!(ports_in("  ➜  Local:   http://localhost:5173/"), [5173]);
        assert_eq!(ports_in("Serving on http://127.0.0.1:8000 and [::1]:8001"), [8000, 8001]);
        assert!(ports_in("remote http://example.com:80 or localhost:0").is_empty());
    }

    /// `npm start` prints its banner long before the server listens: the
    /// start waits for the URL instead of returning on the banner.
    #[cfg(unix)]
    #[tokio::test]
    async fn start_waits_past_a_banner_for_the_url() {
        let root = crate::sandbox::tests::temp("jobs-banner").canonicalize().unwrap();
        let script = "echo '> next start'; sleep 1; echo 'Local: http://localhost:3999'; sleep 60";
        let mut jobs = Jobs::default();
        let first = jobs.start(&Exec::new("/bin/sh".into(), vec!["-c".into(), script.into()], root.clone(), Duration::ZERO), &root).await.unwrap();
        assert!(first.contains("localhost:3999") && jobs.ports().contains(&3999), "{first}");
    }

    /// A server that died on a request: its crash output is found by port.
    #[cfg(unix)]
    #[tokio::test]
    async fn crash_output_is_found_by_port() {
        let root = crate::sandbox::tests::temp("jobs-crash").canonicalize().unwrap();
        let script = "echo 'listening on localhost:3998'; sleep 0.5; echo 'TypeError: boom' >&2; exit 1";
        let mut jobs = Jobs::default();
        jobs.start(&Exec::new("/bin/sh".into(), vec!["-c".into(), script.into()], root.clone(), Duration::ZERO), &root).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(!jobs.ports().contains(&3998));
        let out = jobs.output_on(3998).unwrap();
        assert!(out.contains("exit 1") && out.contains("TypeError: boom"), "{out}");
        assert!(jobs.output_on(4000).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_reports_and_kills_on_stop_and_drop() {
        let root = crate::sandbox::tests::temp("jobs").canonicalize().unwrap();
        let ex = |args: &[&str]| Exec::new("/bin/sh".into(), args.iter().map(|s| s.to_string()).collect(), root.clone(), Duration::ZERO);
        let mut jobs = Jobs::default();

        // A server-like job: prints, then stays up with a child of its own.
        let first = jobs.start(&ex(&["-c", "echo ready; sleep 60 & echo $! ; wait"]), &root).await.unwrap();
        assert!(first.starts_with("job 1 running") && first.contains("ready"), "{first}");
        let pid: libc::pid_t = first.lines().last().unwrap().parse().unwrap();
        assert!(jobs.output(1).unwrap().lines().count() == 1, "old lines repeated");

        let quick = jobs.start(&ex(&["-c", "echo done; exit 3"]), &root).await.unwrap();
        assert!(quick.starts_with("job 2 exit 3") && quick.contains("done"), "{quick}");
        assert_eq!((jobs.running(), jobs.status().lines().count()), (1, 2));
        let status = jobs.status();
        assert!(status.starts_with("job 1 running") && status.contains("s: /bin/sh"), "{status}");

        // A long command: waiting returns as soon as it ends, not at the deadline.
        jobs.start(&ex(&["-c", "echo building; sleep 4; echo built"]), &root).await.unwrap();
        let t = Instant::now();
        let end = jobs.wait(3, 60).await.unwrap();
        assert!(end.starts_with("job 3 exit 0") && end.contains("built") && t.elapsed() < Duration::from_secs(5), "{end}");

        // "Stop it": stop kills a long script and its children at once.
        let long = jobs.start(&ex(&["-c", "sleep 60 & echo $!; wait"]), &root).await.unwrap();
        let child: libc::pid_t = long.lines().last().unwrap().parse().unwrap();
        assert!(jobs.stop(4).unwrap().starts_with("job 4 stopped"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        // SAFETY: signal 0 only checks whether the process exists.
        assert_ne!(unsafe { libc::kill(child, 0) }, 0, "stopped job's child survived");
        assert!(jobs.output(4).is_err());

        // Quitting ai drops the jobs: the server and its child die.
        drop(jobs);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // SAFETY: signal 0 only checks whether the process exists.
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "job child survived");
    }
}
