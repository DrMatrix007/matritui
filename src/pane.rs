use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub struct Pane {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
}

impl Pane {
    pub fn spawn(cmd: &str, size: (u16, u16)) -> std::io::Result<Pane> {
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

        let child = pair
            .slave
            .spawn_command(CommandBuilder::new(cmd))
            .map_err(std::io::Error::other)?;

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
        })
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
