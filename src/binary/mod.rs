//! Contains the binary logic of Ion.
pub mod builtins;
mod completer;
mod designators;
mod history;
mod lexer;
mod prompt;
mod readln;

use ion_shell::{
    builtins::{man_pages, Status},
    expansion::Expander,
    parser::Terminator,
    types, Capture, IonError, PipelineError, Shell, Signal,
};
use itertools::Itertools;
use liner::{Buffer, Context, KeyBindings};
use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    io,
    path::Path,
    rc::Rc,
};
use xdg::BaseDirectories;

#[cfg(not(feature = "advanced_arg_parsing"))]
pub const MAN_ION: &str = r#"Ion - The Ion Shell 1.0.0-alpha
Ion is a commandline shell created to be a faster and easier to use alternative to the currently available shells. It is
not POSIX compliant.

USAGE:
    ion [FLAGS] [OPTIONS] [args]...

FLAGS:
    -h, --help           Prints help information
    -i, --interactive    Force interactive mode
    -n, --no-execute     Do not execute any commands, perform only syntax checking
    -x                   Print commands before execution
    -v, --version        Print the version, platform and revision of Ion then exit

OPTIONS:
    -c <command>             Evaluate given commands instead of reading from the commandline
    -o <key_bindings>        Shortcut layout. Valid options: "vi", "emacs"

ARGS:
    <args>...    Script arguments (@args). If the -c option is not specified, the first parameter is taken as a
                 filename to execute"#;

pub(crate) const MAN_HISTORY: &str = r#"NAME
    history - print command history

SYNOPSIS
    history

DESCRIPTION
    Prints the command history."#;

pub struct InteractiveShell<'a> {
    context: Rc<RefCell<Context>>,
    shell:   RefCell<Shell<'a>>,
}

impl<'a> InteractiveShell<'a> {
    const CONFIG_FILE_NAME: &'static str = "initrc";

    pub fn new(shell: Shell<'a>) -> Self {
        let mut context = Context::new();
        context.word_divider_fn = Box::new(word_divide);
        if shell.variables().get_str("HISTFILE_ENABLED").ok() == Some("1".into()) {
            let path = shell.variables().get_str("HISTFILE").expect("shell didn't set HISTFILE");
            if !Path::new(path.as_str()).exists() {
                eprintln!("ion: creating history file at \"{}\"", path);
            }
            let _ = context.history.set_file_name_and_load_history(path.as_str());
        }
        InteractiveShell { context: Rc::new(RefCell::new(context)), shell: RefCell::new(shell) }
    }

    /// Handles commands given by the REPL, and saves them to history.
    pub fn save_command(&self, cmd: &str) {
        if !cmd.ends_with('/')
            && self
                .shell
                .borrow()
                .tilde(cmd)
                .ok()
                .map_or(false, |path| Path::new(&path.as_str()).is_dir())
        {
            self.save_command_in_history(&[cmd, "/"].concat());
        } else {
            self.save_command_in_history(cmd);
        }
    }

    pub fn add_callbacks(&self) {
        let context = self.context.clone();
        self.shell.borrow_mut().set_on_command(Some(Box::new(move |shell, elapsed| {
            // If `RECORD_SUMMARY` is set to "1" (True, Yes), then write a summary of the
            // pipline just executed to the the file and context histories. At the
            // moment, this means record how long it took.
            if Some("1".into()) == shell.variables().get_str("RECORD_SUMMARY").ok() {
                let summary = format!(
                    "#summary# elapsed real time: {}.{:09} seconds",
                    elapsed.as_secs(),
                    elapsed.subsec_nanos()
                );
                println!("{:?}", summary);
                context.borrow_mut().history.push(summary.into()).unwrap_or_else(|err| {
                    eprintln!("ion: history append: {}", err);
                });
            }
        })));
    }

    fn create_config_file(base_dirs: BaseDirectories, file_name: &str) -> Result<(), io::Error> {
        let path = base_dirs.place_config_file(file_name)?;
        OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(())
    }

    /// Creates an interactive session that reads from a prompt provided by
    /// Liner.
    pub fn execute_interactive(self) -> ! {
        let context_bis = self.context.clone();
        let prep_for_exit = &move |shell: &mut Shell<'_>| {
            // context will be sent a signal to commit all changes to the history file,
            // and waiting for the history thread in the background to finish.
            if shell.opts().huponexit {
                shell.resume_stopped();
                shell.background_send(Signal::SIGHUP).expect("Failed to prepare for exit");
            }
            context_bis.borrow_mut().history.commit_to_file();
        };

        let exit = self.shell.borrow().builtins().get("exit").unwrap();
        let exit = &|args: &[types::Str], shell: &mut Shell<'_>| -> Status {
            prep_for_exit(shell);
            exit(args, shell)
        };

        let exec = self.shell.borrow().builtins().get("exec").unwrap();
        let exec = &|args: &[types::Str], shell: &mut Shell<'_>| -> Status {
            prep_for_exit(shell);
            exec(args, shell)
        };

        let context_bis = self.context.clone();
        let history = &move |args: &[types::Str], _shell: &mut Shell<'_>| -> Status {
            if man_pages::check_help(args, MAN_HISTORY) {
                return Status::SUCCESS;
            }

            print!("{}", context_bis.borrow().history.buffers.iter().format("\n"));
            Status::SUCCESS
        };

        let context_bis = self.context.clone();
        let keybindings = &move |args: &[types::Str], _shell: &mut Shell<'_>| -> Status {
            match args.get(1).map(|s| s.as_str()) {
                Some("vi") => {
                    context_bis.borrow_mut().key_bindings = KeyBindings::Vi;
                    Status::SUCCESS
                }
                Some("emacs") => {
                    context_bis.borrow_mut().key_bindings = KeyBindings::Emacs;
                    Status::SUCCESS
                }
                Some(_) => Status::error("Invalid keybindings. Choices are vi and emacs"),
                None => Status::error("keybindings need an argument"),
            }
        };

        // change the lifetime to allow adding local builtins
        let InteractiveShell { context, shell } = self;
        let mut shell = shell.into_inner();
        shell
            .builtins_mut()
            .add("history", history, "Display a log of all commands previously executed")
            .add("keybindings", keybindings, "Change the keybindings")
            .add("exit", exit, "Exits the current session")
            .add("exec", exec, "Replace the shell with the given command.");

        Self::exec_init_file(&mut shell);

        InteractiveShell { context, shell: RefCell::new(shell) }.exec(prep_for_exit)
    }

    fn exec_init_file(shell: &mut Shell) {
        match BaseDirectories::with_prefix("ion") {
            Ok(base_dirs) => match base_dirs.find_config_file(Self::CONFIG_FILE_NAME) {
                Some(initrc) => match fs::File::open(initrc) {
                    Ok(script) => {
                        if let Err(err) = shell.execute_command(std::io::BufReader::new(script)) {
                            eprintln!("ion: {}", err);
                        }
                    }
                    Err(cause) => println!("ion: init file was not found: {}", cause),
                },
                None => {
                    if let Err(err) = Self::create_config_file(base_dirs, Self::CONFIG_FILE_NAME) {
                        eprintln!("ion: could not create config file: {}", err);
                    }
                }
            },
            Err(err) => {
                eprintln!("ion: unable to get base directory: {}", err);
            }
        }
    }

    fn exec<T: Fn(&mut Shell<'_>)>(self, prep_for_exit: &T) -> ! {
        loop {
            let mut lines = std::iter::repeat_with(|| self.readln(prep_for_exit))
                .filter_map(|cmd| cmd)
                .flat_map(|s| s.into_bytes().into_iter().chain(Some(b'\n')));
            match Terminator::new(&mut lines).terminate() {
                Some(command) => {
                    let cmd: &str = &designators::expand_designators(
                        &self.context.borrow(),
                        command.trim_end(),
                    );
                    {
                        let mut shell = self.shell.borrow_mut();
                        shell.unterminated = false;
                        match shell.on_command(&cmd) {
                            Ok(_) => (),
                            Err(IonError::PipelineExecutionError(
                                PipelineError::CommandNotFound(command),
                            )) => {
                                if shell
                                    .fork_function(
                                        Capture::None,
                                        |_| Ok(()),
                                        "COMMAND_NOT_FOUND",
                                        &["ion", &command],
                                    )
                                    .is_err()
                                {
                                    eprintln!("ion: command not found: {}", command);
                                }
                                // Status::COULD_NOT_EXEC
                            }
                            Err(err) => {
                                eprintln!("ion: {}", err);
                                shell.reset_flow();
                            }
                        }
                    }
                    self.save_command(&cmd);
                }
                None => {
                    self.shell.borrow_mut().unterminated = true;
                }
            }
        }
    }

    /// Set the keybindings of the underlying liner context
    pub fn set_keybindings(&mut self, key_bindings: KeyBindings) {
        self.context.borrow_mut().key_bindings = key_bindings;
    }
}

#[derive(Debug)]
struct WordDivide<I>
where
    I: Iterator<Item = (usize, char)>,
{
    iter:       I,
    count:      usize,
    word_start: Option<usize>,
}
impl<I> WordDivide<I>
where
    I: Iterator<Item = (usize, char)>,
{
    #[inline]
    fn check_boundary(&mut self, c: char, index: usize, escaped: bool) -> Option<(usize, usize)> {
        if let Some(start) = self.word_start {
            if c == ' ' && !escaped {
                self.word_start = None;
                Some((start, index))
            } else {
                self.next()
            }
        } else {
            if c != ' ' {
                self.word_start = Some(index);
            }
            self.next()
        }
    }
}
impl<I> Iterator for WordDivide<I>
where
    I: Iterator<Item = (usize, char)>,
{
    type Item = (usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        self.count += 1;
        match self.iter.next() {
            Some((i, '\\')) => {
                if let Some((_, cnext)) = self.iter.next() {
                    self.count += 1;
                    // We use `i` in order to include the backslash as part of the word
                    self.check_boundary(cnext, i, true)
                } else {
                    self.next()
                }
            }
            Some((i, c)) => self.check_boundary(c, i, false),
            None => {
                // When start has been set, that means we have encountered a full word.
                self.word_start.take().map(|start| (start, self.count - 1))
            }
        }
    }
}

fn word_divide(buf: &Buffer) -> Vec<(usize, usize)> {
    // -> impl Iterator<Item = (usize, usize)> + 'a
    WordDivide { iter: buf.chars().cloned().enumerate(), count: 0, word_start: None }.collect() // TODO: return iterator directly :D
}
