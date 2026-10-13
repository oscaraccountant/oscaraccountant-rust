//! The arguments of `oscar` (clap): the hand-written commands, and one command per public
//! operation from the generated table.

use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use clap_complete::Shell;

use crate::error::{CliError, Result};
use crate::generated::{self, Body, Response, ValueKind};
use crate::render::{self, machine};
use crate::session::Globals;

/// The heading of the options every command takes.
const GLOBAL: &str = "Global options";

/// The heading of the output options of an API command.
const OUTPUT: &str = "Output options";

/// The operations the CLI serves with its own command instead of the generated one.
pub const HAND_WRITTEN: [(&str, &str); 2] = [("files", "upload"), ("files", "download")];

/// The whole command line.
pub fn command() -> Command {
    let root = Command::new("oscar")
        .about("The command-line tool of the Oscar accounting API.")
        .long_about(
            "The command-line tool of the Oscar accounting API.\n\n\
             Log in with `oscar login`, then call the API: `oscar invoices list`. The output is a \
             table on a terminal and JSON otherwise (or with --json).\n\n\
             Exit codes: 0 success, 1 API error, 2 usage error, 3 authentication needed, \
             4 network error, 130 stopped with Ctrl-C.",
        )
        .disable_version_flag(true)
        .arg_required_else_help(true)
        .subcommand_required(false)
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::SetTrue)
                .help("Print the version"),
        );
    let root = common(root, true)
        .subcommand(login())
        .subcommand(onboard())
        .subcommand(common(
            Command::new("logout").about("Revoke the login of a profile and forget it"),
            true,
        ))
        .subcommand(common(
            Command::new("whoami").about("Show the user, the organization and the companies"),
            true,
        ))
        .subcommand(auth())
        .subcommand(profiles())
        .subcommand(config())
        .subcommand(api())
        .subcommand(
            Command::new("completion")
                .about("Print the shell completion script")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(value_parser!(Shell))
                        .help("bash, zsh, fish, powershell or elvish"),
                ),
        );
    resources().into_iter().fold(root, Command::subcommand)
}

/// Add the options every command takes. `key_value`: `--api-key <KEY>` for one call (every
/// command but `login`, where `--api-key` is a switch).
fn common(command: Command, key_value: bool) -> Command {
    let command = command
        .arg(
            Arg::new("json")
                .long("json")
                .value_name("FIELDS")
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value("")
                .help_heading(GLOBAL)
                .help("Print JSON, also on a terminal; --json=a,b keeps these fields"),
        )
        .arg(
            Arg::new("profile")
                .long("profile")
                .value_name("NAME")
                .help_heading(GLOBAL)
                .help("The profile [env: OSCAR_PROFILE]"),
        )
        .arg(
            Arg::new("base-url")
                .long("base-url")
                .value_name("URL")
                .help_heading(GLOBAL)
                .help("The base URL of the API [env: OSCAR_BASE_URL]"),
        )
        .arg(
            Arg::new("no-input")
                .long("no-input")
                .action(ArgAction::SetTrue)
                .help_heading(GLOBAL)
                .help("Never ask; a missing value is an error [env: OSCAR_NO_INPUT]"),
        );
    if key_value {
        command.arg(
            Arg::new("api-key")
                .long("api-key")
                .value_name("KEY")
                .help_heading(GLOBAL)
                .help("An API key for this call only, never stored [env: OSCAR_API_KEY]"),
        )
    } else {
        command
    }
}

fn login() -> Command {
    common(
        Command::new("login")
            .about("Log in: in the browser, with a device code, or with an API key")
            .long_about(
                "Log in and store the credential in the OS keychain.\n\n\
                 The default opens the browser. With --device, or when no display is found, \
                 it prints a code to enter on another device. With --api-key, it reads an \
                 organization API key from standard input or a prompt.",
            )
            .arg(
                Arg::new("device")
                    .long("device")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("api-key")
                    .help("Log in with a code on another device"),
            )
            .arg(
                Arg::new("api-key")
                    .long("api-key")
                    .action(ArgAction::SetTrue)
                    .help("Log in with an API key, read from standard input or a prompt"),
            )
            .arg(
                // The login the onboarding runs again after step 1: the method was chosen once.
                Arg::new("browser")
                    .long("browser")
                    .action(ArgAction::SetTrue)
                    .conflicts_with_all(["device", "api-key"])
                    .hide(true)
                    .help("Log in in the browser, with no question"),
            )
            .arg(
                Arg::new("scope")
                    .long("scope")
                    .value_name("SCOPE")
                    .value_parser(["read", "write"])
                    .help("`read` for a read-only login; the default is `write`"),
            )
            .arg(company_arg().help(
                "The default company: its id, registry code or the start of its name; asked on \
                 a terminal when the login sees several",
            )),
        false,
    )
}

/// `oscar onboard`: every terminal step has a flag; the
/// browser steps print their link.
fn onboard() -> Command {
    const STEP1: &str = "Step 1: your company";
    const STEP2: &str = "Step 2: about your business";
    const STEP3: &str = "Step 3: identity";
    const STEP5: &str = "Step 5: plan";
    let language = || {
        Arg::new("language")
            .long("language")
            .value_name("LANG")
            .value_parser(["en", "et"])
            .help(
                "The language of the agreement and of the identity check [default: your language]",
            )
    };
    let company = || {
        company_arg().help(
            "The company: its id, registry code or the start of its name [env: OSCAR_COMPANY]",
        )
    };
    let text = |id: &'static str, name: &'static str, heading: &'static str, help: &'static str| {
        Arg::new(id)
            .long(id)
            .value_name(name)
            .help_heading(heading)
            .help(help)
    };
    let switch = |id: &'static str, heading: &'static str, help: &'static str| {
        Arg::new(id)
            .long(id)
            .action(ArgAction::SetTrue)
            .help_heading(heading)
            .help(help)
    };
    common(
        Command::new("onboard")
            .about("Onboard a company: the steps of the web onboarding, in the terminal")
            .long_about(
                "Onboard a company in the terminal: your company, the questions about your \
                 business, the identity check, the client agreement, the plan and the setup \
                 checklist. It resumes at the first step that is not done.\n\n\
                 On a terminal it asks what it needs. Each answer also has a flag; with no \
                 terminal or with --no-input a missing answer is an error that names the flag. \
                 The identity check and the payment run in the browser: the CLI prints their \
                 link and waits. The client agreement is accepted only after reading it in the \
                 pager, never with a flag.",
            )
            .args_conflicts_with_subcommands(true)
            .arg(company())
            .arg(text("registry-code", "CODE", STEP1, "The registry code of the company (8 digits)"))
            .arg(
                text("books-today", "ANSWER", STEP1, "Does the company keep books today?")
                    .value_parser(["yes", "no"]),
            )
            .arg(text("books-no-reason", "TEXT", STEP1, "Why the company keeps no books yet"))
            .arg(
                text("books-software", "NAME", STEP1, "Where the books are kept")
                    .value_parser(["merit_aktiva", "xolo", "directo", "other", "my_accountant"]),
            )
            .arg(text("books-software-other", "NAME", STEP1, "The name of the other software"))
            .arg(text("website", "URL", STEP1, "The company website").conflicts_with("no-website"))
            .arg(switch("no-website", STEP1, "The company has no website"))
            .arg(switch(
                "confirm-owners",
                STEP1,
                "Confirm in writing the beneficial owners the register gives",
            ))
            .arg(text(
                "answers",
                "FILE",
                STEP2,
                "The answers of the questionnaire: a JSON object, for example {\"cash_usage\": \"no\"}",
            ))
            .arg(
                text("answer", "KEY=VALUE", STEP2, "One answer, for example cash_usage=no or customer_countries=EE,FI")
                    .action(ArgAction::Append),
            )
            .arg(switch(
                "consent",
                STEP3,
                "You read the privacy notice and the Didit Verification Privacy Notice",
            ))
            .arg(
                text("pep", "ANSWER", STEP3, "Do you hold, or have you held, a prominent public function?")
                    .value_parser(["yes", "no"]),
            )
            .arg(
                text("pep-related", "ANSWER", STEP3, "Is a family member or close associate of yours a PEP?")
                    .value_parser(["yes", "no"]),
            )
            .arg(text("pep-details", "TEXT", STEP3, "Which function, and who holds it"))
            .arg(switch(
                "invite-links",
                STEP3,
                "Make an invite link for each other person who must verify",
            ))
            .arg(language())
            .arg(
                text("plan", "PLAN", STEP5, "The plan: month or year")
                    .value_parser(["month", "year"]),
            )
            .arg(
                Arg::new("no-wait")
                    .long("no-wait")
                    .action(ArgAction::SetTrue)
                    .help("Print the link of a browser step and stop, rather than wait"),
            )
            .subcommand(common(
                Command::new("status")
                    .about("Show the steps of the onboarding and where it stands")
                    .arg(company()),
                true,
            ))
            .subcommand(common(
                Command::new("agreement")
                    .about("Read and accept the client agreement, or save the evidence PDF")
                    .arg(company())
                    .arg(language())
                    .arg(
                        Arg::new("download")
                            .long("download")
                            .value_name("PATH")
                            .num_args(0..=1)
                            .default_missing_value("")
                            .help("Save the PDF of the accepted agreement [default: client-agreement-<company>.pdf]"),
                    )
                    .arg(
                        Arg::new("force")
                            .long("force")
                            .action(ArgAction::SetTrue)
                            .requires("download")
                            .help("Replace an existing file with --download"),
                    ),
                true,
            )),
        true,
    )
}

fn auth() -> Command {
    Command::new("auth")
        .about("The credential of the CLI")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(common(
            Command::new("token")
                .about("Print a fresh access token, or the API key")
                .long_about(
                    "Print a fresh access token of the profile, or its API key. With --json: \
                     {\"token\", \"expires_at\", \"kind\", \"base_url\", \"profile\"}. With \
                     --base-url, only a profile of that base URL answers.",
                ),
            false,
        ))
        .subcommand(common(
            Command::new("status").about("Show the state of the login"),
            false,
        ))
}

fn profiles() -> Command {
    Command::new("profiles")
        .about("The profiles: one login each")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(common(
            Command::new("list").about("List the profiles"),
            false,
        ))
        .subcommand(common(
            Command::new("use")
                .about("Make a profile the default")
                .arg(Arg::new("name").required(true).value_name("NAME")),
            false,
        ))
}

fn config() -> Command {
    let key = || {
        Arg::new("key")
            .required(true)
            .value_name("KEY")
            .value_parser(["company"])
            .help("The setting: `company`")
    };
    Command::new("config")
        .about("The settings of a profile")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(common(
            Command::new("get").about("Print a setting").arg(key()),
            false,
        ))
        .subcommand(common(
            Command::new("set")
                .about("Change a setting")
                .arg(key())
                .arg(Arg::new("value").required(true).value_name("VALUE")),
            false,
        ))
}

fn data_arg(required: bool) -> Arg {
    Arg::new("data")
        .long("data")
        .short('d')
        .value_name("JSON")
        .required(required)
        .help("The JSON body: '<json>', @file.json, or - for standard input")
}

fn idempotency_arg() -> Arg {
    Arg::new("idempotency-key")
        .long("idempotency-key")
        .value_name("KEY")
        .help("The Idempotency-Key; the CLI makes one when it is not given")
}

fn api() -> Command {
    common(
        Command::new("api")
            .about("Call any path of the API with the stored credential")
            .long_about(
                "Call any path of the API with the stored credential, like `gh api`, for an \
                 endpoint the CLI has no command for yet. The path is relative to the base URL \
                 and may hold a query: `oscar api GET '/api/me/'`.",
            )
            .arg(
                Arg::new("method")
                    .required(true)
                    .value_name("METHOD")
                    .help("GET, POST, PUT, PATCH or DELETE"),
            )
            .arg(Arg::new("path").required(true).value_name("PATH"))
            .arg(data_arg(false))
            .arg(idempotency_arg()),
        true,
    )
}

/// The value name of a kind of value.
fn value_name(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Integer => "N",
        ValueKind::Number | ValueKind::Decimal => "NUMBER",
        ValueKind::Boolean => "BOOL",
        ValueKind::Date => "YYYY-MM-DD",
        ValueKind::DateTime => "DATETIME",
        ValueKind::Email => "EMAIL",
        ValueKind::Enum(_) => "VALUE",
        ValueKind::File => "PATH",
        ValueKind::String => "TEXT",
    }
}

/// The help of a flag, with its values when they are a set.
fn flag_help(flag: &generated::Flag) -> String {
    let help = flag
        .help
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    match flag.kind {
        ValueKind::Enum(values) => {
            let values = values.join(", ");
            if help.is_empty() {
                format!("One of: {values}")
            } else {
                format!("{help} One of: {values}")
            }
        }
        _ => help,
    }
}

fn flag_arg(id_prefix: &str, flag: &generated::Flag) -> Arg {
    Arg::new(format!("{id_prefix}:{}", flag.name))
        .long(flag.long)
        .value_name(value_name(flag.kind))
        .required(flag.required)
        .help(flag_help(flag))
}

/// The id of the argument of a path parameter.
pub fn path_id(name: &str) -> String {
    match name {
        "company_id" => "company".to_owned(),
        "organization_id" => "organization".to_owned(),
        other => format!("path:{other}"),
    }
}

/// The argument of a path parameter: `--company` and `--organization` have a default, every
/// other one is a positional argument.
fn path_arg(operation: &generated::Command, arg: &generated::Arg) -> Arg {
    let help = id_help(operation, arg.name).unwrap_or_else(|| {
        arg.help
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    });
    match arg.name {
        "company_id" => company_arg().help(
            "The company: its id, registry code or the start of its name [default: the \
             profile's; env: OSCAR_COMPANY]",
        ),
        "organization_id" => Arg::new("organization")
            .long("organization")
            .value_name("ID")
            .help("The organization [default: the profile's]"),
        name => Arg::new(path_id(name))
            .required(true)
            .value_name(name.to_uppercase())
            .help(help),
    }
}

/// The help of an id the CLI resolves against the list of its place (`invoices get`): the id,
/// its start or end, or the title the list shows (an invoice number, a file name). `None` when
/// no list resolves it: the help of the schema stays (N3-e).
fn id_help(operation: &generated::Command, name: &str) -> Option<String> {
    let template: Vec<&str> = operation.path.trim_end_matches('/').split('/').collect();
    let index = template
        .iter()
        .position(|segment| *segment == format!("{{{name}}}"))?;
    let list_path = template[..index].join("/") + "/";
    let list = generated::COMMANDS
        .iter()
        .find(|other| other.paginated && other.path == list_path)?;
    let title = list
        .view
        .map(|view| view.title)
        .filter(|title| !title.is_empty() && *title != "id");
    Some(match title {
        Some(title) => format!(
            "The {} as the list shows it, the id, or the start or end of the id",
            title.replace('_', " ")
        ),
        None => "The id, or the start or end of the id".to_owned(),
    })
}

/// `-c/--company <x>`: a company by its id, its registry code or the start of its name.
fn company_arg() -> Arg {
    Arg::new("company")
        .short('c')
        .long("company")
        .value_name("COMPANY")
}

/// `oscar companies use <x>`: the hand-written command of the group `companies`.
fn companies_use() -> Command {
    common(
        Command::new("use")
            .about("Make a company the default of the profile")
            .long_about(
                "Make a company the default of the profile. <COMPANY> is its id, its registry \
                 code, or the start of its name (any case).",
            )
            .arg(
                Arg::new("company")
                    .required(true)
                    .value_name("COMPANY")
                    .help("The id, the registry code, or the start of the name"),
            ),
        false,
    )
}

/// One command per public operation, grouped by resource. An operation with no CLI name (a
/// retrieve with no id) is the resource itself: `oscar trial-balance`; it keeps `retrieve` as a
/// hidden subcommand.
fn resources() -> Vec<Command> {
    let mut groups: Vec<Command> = Vec::new();
    for operation in generated::COMMANDS {
        let command = if HAND_WRITTEN.contains(&(operation.resource, operation.method)) {
            hand_written(operation)
        } else {
            operation_command(operation)
        };
        let position = groups
            .iter()
            .position(|group| group.get_name() == operation.resource);
        let mut group = match position {
            Some(index) => groups.remove(index),
            None => Command::new(operation.resource)
                .subcommand_required(true)
                .arg_required_else_help(true),
        };
        if operation.name.is_empty() {
            group = itself(group, operation, command);
        } else {
            group = group.subcommand(command);
        }
        match position {
            Some(index) => groups.insert(index, group),
            None => groups.push(group),
        }
    }
    groups
        .into_iter()
        .map(|group| {
            let group = if group.get_name() == "companies" {
                group.subcommand(companies_use())
            } else {
                group
            };
            // A resource that is a command itself keeps the summary of its operation.
            if group.get_about().is_some() {
                group
            } else {
                let about = group_about(&group);
                group.about(about)
            }
        })
        .collect()
}

/// The help line of a resource group: its name and its commands, as `VAT codes: list` or
/// `Invoices: list, get, record, issue, revise, cancel`.
pub fn group_about(group: &Command) -> String {
    let words = group
        .get_name()
        .split('-')
        .map(|word| match word {
            "vat" => "VAT".to_owned(),
            other => other.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut title = words.chars();
    let title = title.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(title).collect()
    });
    format!("{title}: {}", visible_subcommands(group).join(", "))
}

/// The names of the subcommands a person sees, in their order: no hidden alias, no hidden
/// command.
pub fn visible_subcommands(group: &Command) -> Vec<&str> {
    group
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .map(Command::get_name)
        .collect()
}

/// The resource group that also runs the operation itself: its arguments on the group, the
/// operation as a hidden `retrieve` subcommand.
fn itself(group: Command, operation: &'static generated::Command, command: Command) -> Command {
    let mut group = group
        .about(operation.summary)
        .long_about(operation.description)
        .subcommand_required(false)
        .arg_required_else_help(false)
        .args_conflicts_with_subcommands(true);
    for arg in command.get_arguments() {
        group = group.arg(arg.clone());
    }
    group.subcommand(command.hide(true))
}

/// `-o`, `--columns`, `--jq` and `--template`: how an API command prints its answer.
fn output_args(command: Command) -> Command {
    command
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("FORMAT")
                .value_parser(["table", "wide", "json", "ndjson"])
                .help_heading(OUTPUT)
                .help(
                    "table (the default on a terminal), wide, json, or ndjson (one item per line)",
                ),
        )
        .arg(
            Arg::new("columns")
                .long("columns")
                .value_name("LIST")
                .help_heading(OUTPUT)
                .help("The columns of a list, in order: --columns number,due_date"),
        )
        .arg(
            Arg::new("jq")
                .long("jq")
                .value_name("EXPR")
                .help_heading(OUTPUT)
                .help("Filter the JSON with a jq expression"),
        )
        .arg(
            Arg::new("template")
                .long("template")
                .value_name("TEXT")
                .conflicts_with("jq")
                .help_heading(OUTPUT)
                .help("Print with a template (minijinja): '{{ number }} {{ state }}'"),
        )
}

/// The command of an operation: `oscar <resource> <name>`, with its SDK method as a hidden alias.
fn operation_command(operation: &'static generated::Command) -> Command {
    let name = if operation.name.is_empty() {
        operation.method
    } else {
        operation.name
    };
    let mut command = Command::new(name)
        .about(operation.summary)
        .long_about(operation.description);
    if name != operation.method {
        command = command.alias(operation.method);
    }
    for arg in operation.args {
        command = command.arg(path_arg(operation, arg));
    }
    for flag in operation.flags {
        command = command.arg(flag_arg("query", flag));
    }
    if let Body::Json { required, .. } = operation.body {
        command = command.arg(data_arg(required));
    }
    if operation.idempotent {
        command = command.arg(idempotency_arg());
    }
    if operation.paginated {
        let mut all = Arg::new("all")
            .long("all")
            .action(ArgAction::SetTrue)
            .help("Walk every page and print every item");
        if operation.flags.iter().any(|flag| flag.name == "cursor") {
            all = all.conflicts_with("query:cursor");
        }
        command = command.arg(all);
    }
    if operation.response == Response::Json {
        command = output_args(command);
    }
    common(command, true)
}

/// `files upload <PATH>` and `files download <FILE_ID>`: the same arguments as the operation,
/// with the file as a path.
fn hand_written(operation: &'static generated::Command) -> Command {
    let mut command = Command::new(operation.method)
        .about(operation.summary)
        .long_about(operation.description);
    if operation.method == "upload" {
        command = command.arg(
            Arg::new("file")
                .required(true)
                .value_name("PATH")
                .help("The file to upload"),
        );
    }
    for arg in operation.args {
        command = command.arg(path_arg(operation, arg));
    }
    if let Body::Multipart { fields } = operation.body {
        for field in fields.iter().filter(|field| field.kind != ValueKind::File) {
            command = command.arg(flag_arg("field", field));
        }
    }
    if operation.idempotent {
        command = command.arg(idempotency_arg());
    }
    if operation.method == "download" {
        command = command
            .arg(
                Arg::new("output")
                    .short('o')
                    .long("output")
                    .value_name("PATH")
                    .help(
                        "Where to write the file; - for standard output [default: the file's name]",
                    ),
            )
            .arg(
                Arg::new("force")
                    .long("force")
                    .action(ArgAction::SetTrue)
                    .help("Replace an existing file"),
            );
    }
    common(command, true)
}

/// The options every command takes, from the leaf command and the root: the leaf wins.
pub fn globals(root: &ArgMatches, leaf: &ArgMatches) -> Globals {
    let text = |id: &str| {
        string(leaf, id)
            .or_else(|| string(root, id))
            .map(str::to_owned)
    };
    Globals {
        json: string(leaf, "json").is_some() || string(root, "json").is_some(),
        profile: text("profile"),
        base_url: text("base-url"),
        api_key: string(leaf, "api-key")
            .or_else(|| string(root, "api-key"))
            .map(str::to_owned),
        output: output_options(root, leaf),
        no_input: flag(leaf, "no-input") || flag(root, "no-input"),
    }
}

/// The output options of the leaf command, and `--json` of the root.
pub fn output_options(root: &ArgMatches, leaf: &ArgMatches) -> render::Options {
    let fields = string(leaf, "json")
        .filter(|fields| !fields.is_empty())
        .or_else(|| string(root, "json"))
        .map(machine::field_list)
        .unwrap_or_default();
    render::Options {
        format: string(leaf, "output").and_then(render::Format::parse),
        json: string(leaf, "json").is_some() || string(root, "json").is_some(),
        fields,
        columns: string(leaf, "columns").map(machine::field_list),
        jq: string(leaf, "jq").map(str::to_owned),
        template: string(leaf, "template").map(str::to_owned),
    }
}

/// The check of the output options that clap cannot make: `--columns` with an empty list.
pub fn check_output(options: &render::Options) -> Result<()> {
    if options.columns.as_ref().is_some_and(Vec::is_empty) {
        return Err(CliError::usage("--columns needs at least one column."));
    }
    Ok(())
}

/// The value of a string argument, when the command has it and it was given.
pub fn string<'a>(matches: &'a ArgMatches, id: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(id)
        .ok()
        .flatten()
        .map(String::as_str)
}

/// A switch, `false` when the command does not have it.
pub fn flag(matches: &ArgMatches, id: &str) -> bool {
    matches
        .try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> std::result::Result<ArgMatches, clap::Error> {
        command().try_get_matches_from(args)
    }

    #[test]
    fn the_command_is_valid() {
        command().debug_assert();
    }

    #[test]
    fn a_group_names_its_commands() {
        let root = command();
        let about = |name: &str| {
            root.find_subcommand(name)
                .and_then(Command::get_about)
                .map(ToString::to_string)
                .unwrap()
        };
        assert_eq!(about("vat-codes"), "VAT codes: list");
        assert_eq!(about("bank-transactions"), "Bank transactions: list, get");
        assert_eq!(about("companies"), "Companies: list, get, use");
        assert_eq!(
            about("me"),
            "Show the user, the organization and the companies"
        );
        assert_eq!(group_about(&Command::new("")), ": ");
        let issue = root
            .find_subcommand("invoices")
            .and_then(|group| group.find_subcommand("issue"))
            .unwrap();
        let help = issue
            .get_arguments()
            .find(|arg| arg.get_id() == "path:invoice_id")
            .and_then(Arg::get_help)
            .map(ToString::to_string);
        assert_eq!(
            help.as_deref(),
            Some("The number as the list shows it, the id, or the start or end of the id")
        );
    }

    #[test]
    fn has_one_command_per_operation() {
        let root = command();
        for operation in generated::COMMANDS {
            let group = root
                .find_subcommand(operation.resource)
                .unwrap_or_else(|| panic!("{}", operation.resource));
            assert!(
                group.find_subcommand(operation.method).is_some(),
                "{}",
                operation.method
            );
        }
    }

    #[test]
    fn parses_an_operation() {
        let matches = parse(&[
            "oscar",
            "invoices",
            "list",
            "--state",
            "issued",
            "--limit",
            "5",
            "--all",
            "--company",
            "c1",
            "--json",
        ])
        .unwrap();
        let (group, matches) = matches.subcommand().unwrap();
        assert_eq!(group, "invoices");
        let (method, leaf) = matches.subcommand().unwrap();
        assert_eq!(method, "list");
        assert_eq!(string(leaf, "query:state"), Some("issued"));
        assert_eq!(string(leaf, "company"), Some("c1"));
        assert!(flag(leaf, "all"));
        assert_eq!(string(leaf, "json"), Some(""));
        assert!(parse(&["oscar", "invoices", "list", "--all", "--cursor", "x"]).is_err());
        let matches = parse(&["oscar", "invoices", "retrieve", "inv1"]).unwrap();
        let leaf = matches.subcommand().unwrap().1.subcommand().unwrap().1;
        assert_eq!(string(leaf, "path:invoice_id"), Some("inv1"));
        assert!(
            parse(&["oscar", "invoices", "record"]).is_err(),
            "--data is required"
        );
    }

    #[test]
    fn merges_the_global_options() {
        let root = parse(&[
            "oscar",
            "--json",
            "--profile",
            "p",
            "--api-key",
            "osk_root",
            "whoami",
            "--profile",
            "q",
        ])
        .unwrap();
        let leaf = root.subcommand().unwrap().1;
        let globals = globals(&root, leaf);
        assert_eq!(
            globals,
            Globals {
                json: true,
                profile: Some("q".into()),
                base_url: None,
                api_key: Some("osk_root".into()),
                output: render::Options {
                    json: true,
                    ..render::Options::default()
                },
                no_input: false,
            }
        );
        let root = parse(&["oscar", "login", "--api-key"]).unwrap();
        let leaf = root.subcommand().unwrap().1;
        assert!(flag(leaf, "api-key"));
        assert_eq!(super::globals(&root, leaf).api_key, None);
        assert_eq!(string(leaf, "nothing"), None);
        assert!(!flag(leaf, "nothing"));
    }

    #[test]
    fn the_file_commands() {
        let matches = parse(&[
            "oscar",
            "files",
            "upload",
            "a.pdf",
            "--kind",
            "invoice",
            "--received-on",
            "2026-09-01",
        ])
        .unwrap();
        let leaf = matches.subcommand().unwrap().1.subcommand().unwrap().1;
        assert_eq!(string(leaf, "file"), Some("a.pdf"));
        assert_eq!(string(leaf, "field:kind"), Some("invoice"));
        assert_eq!(string(leaf, "field:received_on"), Some("2026-09-01"));
        let matches = parse(&["oscar", "files", "download", "f1", "-o", "-", "--force"]).unwrap();
        let leaf = matches.subcommand().unwrap().1.subcommand().unwrap().1;
        assert_eq!(string(leaf, "path:file_id"), Some("f1"));
        assert_eq!(string(leaf, "output"), Some("-"));
    }

    #[test]
    fn helps() {
        assert_eq!(path_id("company_id"), "company");
        assert_eq!(path_id("organization_id"), "organization");
        assert_eq!(path_id("invoice_id"), "path:invoice_id");
        for (kind, name) in [
            (ValueKind::Integer, "N"),
            (ValueKind::Number, "NUMBER"),
            (ValueKind::Decimal, "NUMBER"),
            (ValueKind::Boolean, "BOOL"),
            (ValueKind::Date, "YYYY-MM-DD"),
            (ValueKind::DateTime, "DATETIME"),
            (ValueKind::Email, "EMAIL"),
            (ValueKind::Enum(&[]), "VALUE"),
            (ValueKind::File, "PATH"),
            (ValueKind::String, "TEXT"),
        ] {
            assert_eq!(value_name(kind), name);
        }
        let flag = generated::Flag {
            name: "state",
            long: "state",
            help: "",
            required: false,
            kind: ValueKind::Enum(&["a", "b"]),
        };
        assert_eq!(flag_help(&flag), "One of: a, b");
        let flag = generated::Flag {
            help: "The state.",
            ..flag
        };
        assert_eq!(flag_help(&flag), "The state. One of: a, b");
        assert!(parse(&["oscar"]).is_err());
        assert!(parse(&["oscar", "completion", "bash"]).is_ok());
    }
}
