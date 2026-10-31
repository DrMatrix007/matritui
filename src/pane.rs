use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub struct Pane {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    hook_id: String,
    settings_path: Option<PathBuf>,
    cwd: PathBuf,
}

impl Pane {
    pub fn spawn(cmd: &str, size: (u16, u16), cwd: &Path) -> std::io::Result<Pane> {
        Self::spawn_from_builder(CommandBuilder::new(cmd), size, String::new(), None, cwd)
    }

    pub fn spawn_with_hooks(
        cmd: &str,
        size: (u16, u16),
        hook_id: String,
        settings_path: PathBuf,
        env: Vec<(String, String)>,
        args: Vec<String>,
        cwd: &Path,
    ) -> std::io::Result<Pane> {
        let mut builder = CommandBuilder::new(cmd);
        builder.args(args);
        for (key, value) in env {
            builder.env(key, value);
        }

        Self::spawn_from_builder(builder, size, hook_id, Some(settings_path), cwd)
    }

    fn spawn_from_builder(
        mut builder: CommandBuilder,
        size: (u16, u16),
        hook_id: String,
        settings_path: Option<PathBuf>,
        cwd: &Path,
    ) -> std::io::Result<Pane> {
        builder.cwd(cwd.as_os_str());

        let (rows, cols) = size;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)?;

        let child = pair.slave.spawn_command(builder).map_err(std::io::Error::other)?;

        let writer = pair.master.take_writer().map_err(std::io::Error::other)?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(std::io::Error::other)?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
            0,
        )));

        let parser_for_reader = Arc::clone(&parser);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => parser_for_reader.lock().unwrap().process(&buf[..n]),
                }
            }
        });

        Ok(Pane {
            master: pair.master,
            child,
            writer,
            parser,
            hook_id,
            settings_path,
            cwd: cwd.to_path_buf(),
        })
    }

    pub fn hook_id(&self) -> &str {
        &self.hook_id
    }

    pub fn is_agent(&self) -> bool {
        self.settings_path.is_some()
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)?;

        self.parser
            .lock()
            .unwrap()
            .set_size(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap());

        Ok(())
    }

    pub fn try_wait_exit(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        self.child.try_wait()
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    pub fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        self.child.wait()
    }

    pub fn parser(&self) -> &Arc<Mutex<vt100::Parser>> {
        &self.parser
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let Some(path) = &self.settings_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_with_hooks_stores_given_hook_id() {
        let mut pane_a = Pane::spawn_with_hooks(
            "cat",
            (24, 80),
            "test-pane-a".to_string(),
            PathBuf::from("/tmp/mmux-pane-test-a.json"),
            vec![],
            vec![],
            std::env::current_dir().unwrap().as_path(),
        )
        .unwrap();
        let mut pane_b = Pane::spawn_with_hooks(
            "cat",
            (24, 80),
            "test-pane-b".to_string(),
            PathBuf::from("/tmp/mmux-pane-test-b.json"),
            vec![],
            vec![],
            std::env::current_dir().unwrap().as_path(),
        )
        .unwrap();

        assert_eq!(pane_a.hook_id(), "test-pane-a");
        assert_eq!(pane_b.hook_id(), "test-pane-b");
        assert_ne!(pane_a.hook_id(), pane_b.hook_id());

        pane_a.kill().unwrap();
        pane_b.kill().unwrap();
    }

    #[test]
    fn spawn_captures_current_dir_as_cwd() {
        let mut pane = Pane::spawn("cat", (24, 80), &std::env::current_dir().unwrap()).unwrap();

        assert_eq!(pane.cwd(), std::env::current_dir().unwrap());

        pane.kill().unwrap();
    }

    #[test]
    fn spawn_with_explicit_cwd_different_from_process_cwd_is_used() {
        let explicit_cwd = std::env::temp_dir();
        let mut pane = Pane::spawn("cat", (24, 80), &explicit_cwd).unwrap();

        assert_eq!(pane.cwd(), explicit_cwd);
        assert_ne!(pane.cwd(), std::env::current_dir().unwrap());

        pane.kill().unwrap();
    }

    #[test]
    fn spawn_is_not_an_agent_pane() {
        let mut pane = Pane::spawn("cat", (24, 80), &std::env::current_dir().unwrap()).unwrap();

        assert!(!pane.is_agent());

        pane.kill().unwrap();
    }

    #[test]
    fn spawn_with_hooks_is_an_agent_pane() {
        let mut pane = Pane::spawn_with_hooks(
            "cat",
            (24, 80),
            "test-pane-agent".to_string(),
            PathBuf::from("/tmp/mmux-pane-test-agent.json"),
            vec![],
            vec![],
            &std::env::current_dir().unwrap(),
        )
        .unwrap();

        assert!(pane.is_agent());

        pane.kill().unwrap();
    }
}
