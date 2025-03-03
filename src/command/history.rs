use std::env;
use std::io::Write;
use std::time::Duration;

use eyre::Result;
use structopt::StructOpt;
use tabwriter::TabWriter;

use atuin_client::database::Database;
use atuin_client::history::History;
use atuin_client::settings::Settings;
use atuin_client::sync;

#[derive(StructOpt)]
pub enum Cmd {
    #[structopt(
        about="begins a new command in the history",
        aliases=&["s", "st", "sta", "star"],
    )]
    Start { command: Vec<String> },

    #[structopt(
        about="finishes a new command in the history (adds time, exit code)",
        aliases=&["e", "en"],
    )]
    End {
        id: String,
        #[structopt(long, short)]
        exit: i64,
    },

    #[structopt(
        about="list all items in history",
        aliases=&["l", "li", "lis"],
    )]
    List {
        #[structopt(long, short)]
        cwd: bool,

        #[structopt(long, short)]
        session: bool,

        #[structopt(long, short)]
        human: bool,

        #[structopt(long, help = "Show only the text of the command")]
        cmd_only: bool,
    },

    #[structopt(
        about="get the last command ran",
        aliases=&["la", "las"],
    )]
    Last {
        #[structopt(long, short)]
        human: bool,

        #[structopt(long, help = "Show only the text of the command")]
        cmd_only: bool,
    },
}

#[allow(clippy::cast_sign_loss)]
pub fn print_list(h: &[History], human: bool, cmd_only: bool) {
    let mut writer = TabWriter::new(std::io::stdout()).padding(2);

    let lines = h.iter().map(|h| {
        if human {
            let duration = humantime::format_duration(Duration::from_nanos(std::cmp::max(
                h.duration, 0,
            ) as u64))
            .to_string();
            let duration: Vec<&str> = duration.split(' ').collect();
            let duration = duration[0];

            format!(
                "{}\t{}\t{}\n",
                h.timestamp.format("%Y-%m-%d %H:%M:%S"),
                h.command.trim(),
                duration,
            )
        } else if cmd_only {
            format!("{}\n", h.command.trim())
        } else {
            format!(
                "{}\t{}\t{}\n",
                h.timestamp.timestamp_nanos(),
                h.command.trim(),
                h.duration
            )
        }
    });

    for i in lines.rev() {
        writer
            .write_all(i.as_bytes())
            .expect("failed to write to tab writer");
    }

    writer.flush().expect("failed to flush tab writer");
}

impl Cmd {
    pub async fn run(
        &self,
        settings: &Settings,
        db: &mut (impl Database + Send + Sync),
    ) -> Result<()> {
        match self {
            Self::Start { command: words } => {
                let command = words.join(" ");

                if command.starts_with(' ') {
                    return Ok(());
                }

                let cwd = env::current_dir()?.display().to_string();

                let h = History::new(chrono::Utc::now(), command, cwd, -1, -1, None, None);

                // print the ID
                // we use this as the key for calling end
                println!("{}", h.id);
                db.save(&h).await?;
                Ok(())
            }

            Self::End { id, exit } => {
                if id.trim() == "" {
                    return Ok(());
                }

                let mut h = db.load(id).await?;

                if h.duration > 0 {
                    debug!("cannot end history - already has duration");

                    // returning OK as this can occur if someone Ctrl-c a prompt
                    return Ok(());
                }

                h.exit = *exit;
                h.duration = chrono::Utc::now().timestamp_nanos() - h.timestamp.timestamp_nanos();

                db.update(&h).await?;

                if settings.should_sync()? {
                    debug!("running periodic background sync");
                    sync::sync(settings, false, db).await?;
                } else {
                    debug!("sync disabled! not syncing");
                }

                Ok(())
            }

            Self::List {
                session,
                cwd,
                human,
                cmd_only,
            } => {
                let session = if *session {
                    Some(env::var("ATUIN_SESSION")?)
                } else {
                    None
                };
                let cwd = if *cwd {
                    Some(env::current_dir()?.display().to_string())
                } else {
                    None
                };

                let history = match (session, cwd) {
                    (None, None) => db.list(None, false).await?,
                    (None, Some(cwd)) => {
                        let query = format!("select * from history where cwd = {};", cwd);
                        db.query_history(&query).await?
                    }
                    (Some(session), None) => {
                        let query = format!("select * from history where session = {};", session);
                        db.query_history(&query).await?
                    }
                    (Some(session), Some(cwd)) => {
                        let query = format!(
                            "select * from history where cwd = {} and session = {};",
                            cwd, session
                        );
                        db.query_history(&query).await?
                    }
                };

                print_list(&history, *human, *cmd_only);

                Ok(())
            }

            Self::Last { human, cmd_only } => {
                let last = db.last().await?;
                print_list(&[last], *human, *cmd_only);

                Ok(())
            }
        }
    }
}
