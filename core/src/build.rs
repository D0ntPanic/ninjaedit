//! Build configuration: how a project is built and run, kept in
//! `build.toml` in the project's [storage](crate::storage) directory.
//!
//! A project has any number of build **roots**, each a `Cargo.toml` or a
//! `CMakeLists.txt` somewhere in the tree (a root at the top of the
//! project is found by itself; nested subprojects are added by hand).
//! Under each root are two lists:
//!
//! * **Configurations** are the ways of building: `dev` and `release`
//!   for Cargo, `Debug` and `Release` for CMake, and whatever else the
//!   user makes. A configuration carries the options that differ between
//!   builds of the same thing: the Cargo profile, the `CMAKE_BUILD_TYPE`,
//!   extra arguments for the configure and build commands, and
//!   environment variables set while building.
//! * **Targets** are the things to build and run: a Cargo package (found
//!   from the manifest, workspace members included) or a CMake target,
//!   with what is needed to run the result: the executable, its working
//!   directory, arguments, and environment.
//!
//! When a Cargo root is first set up, the target it starts on is the
//! package most likely meant to be run: one with a binary, and among
//! those one the workspace's `default-members` names, with the root
//! package and then the members' order breaking ties.
//!
//! Targets found by looking at the project (a manifest's packages, a
//! CMake tree's executables) are **discovered** targets, and the project
//! stays their source of truth: discovery runs again every time the
//! configuration is loaded, so a package added to the workspace shows up
//! by itself and one taken out goes away. Looking at a big project takes
//! a while (every `CMakeLists.txt` in the tree is read), so discovery is
//! a piece of work a frontend runs on a thread of its own: a root
//! **pending** discovery has only what the file gave it and what costs
//! nothing to know (a CMake root's `All`), [`BuildConfig::request_discovery`]
//! hands out the work, [`Discovery::run`] does it anywhere, and
//! [`BuildConfig::apply_discovery`] takes the result in, with the current
//! target kept by name across the change. A frontend that would rather
//! wait has [`BuildConfig::sync_discovered`] and [`BuildConfig::discover`],
//! which do it all on the spot. The file records only what the user did to them: a
//! discovered target with its default options isn't written at all, one
//! whose options were changed is written with its origin so the changes
//! survive, and one whose source has since gone keeps the changes as an
//! ordinary target. A discovered target can't be removed, since it
//! would only come back; it is **disabled** instead, which keeps it in
//! the list (crossed out, for a frontend) but out of the way of
//! building, until it is enabled again.
//!
//! Cargo keeps its own build directory and this module leaves it to it.
//! A CMake configuration builds in `cmake-build-<name>` beside its
//! `CMakeLists.txt`, the name lowercased, the way CLion does, so one
//! `.gitignore` line covers every configuration. The configure step uses
//! the generator from the settings (Ninja unless changed), except where
//! a configuration's own configure arguments name one with `-G`.
//!
//! The project has a **current** configuration and target, the ones a
//! build (Ctrl+B) and a run (Ctrl+R) use. Both belong to one root; picking
//! a configuration from another root brings the target along. Any other
//! target can be run without becoming current
//! ([`BuildConfig::selection_with_target`] says what it runs with,
//! [`BuildConfig::run_job_for`] runs it): a benchmark or a test suite
//! that is wanted now and then, while the target being worked on stays
//! a keystroke away.
//!
//! What a configuration has built can be thrown away to start over
//! ([`BuildConfig::clean_job`] for the current one,
//! [`BuildConfig::clean_all_job`] for every root): a CMake build
//! directory is deleted, and Cargo is asked to clean the profile (or,
//! for every root, its whole target directory).
//!
//! Frontends edit the lists through [`BuildConfig`]'s text methods, the
//! way the settings are edited: each option is a text field, and
//! [`ConfigurationKey`] and [`TargetKey`] list the options each build
//! system shows with their names and descriptions. What a build actually
//! runs comes out of [`BuildConfig::build_job`] and
//! [`BuildConfig::run_job`] as a list of commands to run one after
//! another.

use crate::settings::Settings;
use crate::terminal::Command;
use regex::Regex;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use toml::{Table, Value};

/// The build configuration file's name within a project's storage.
pub const BUILD_FILE: &str = "build.toml";
/// What a CMake configuration's build directory is called, before the
/// configuration's name.
pub const CMAKE_BUILD_DIR_PREFIX: &str = "cmake-build-";
/// The generator CMake configures with unless the settings say otherwise.
pub const DEFAULT_CMAKE_GENERATOR: &str = "Ninja";
/// Why nothing can be built with no roots: what a job asked of a
/// configuration without a selection says.
pub const NO_ROOT_MESSAGE: &str =
    "No build root: add a Cargo.toml or CMakeLists.txt on the build configuration page";

const FILE_HEADER: &str = "# ninjaedit build configuration for this project.\n";
/// The file keys that say a target was discovered, and as what, and that
/// it is disabled.
const AUTO_KEY: &str = "auto";
const DISABLED_KEY: &str = "disabled";
/// The origin of the discovered target that builds everything under a
/// root: a Cargo manifest without packages, or a CMake tree's `all`.
const ALL_TARGET_ID: &str = "*";

// ----- Build systems --------------------------------------------------------

/// A build system a root belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildSystem {
    Cargo,
    CMake,
}

impl BuildSystem {
    pub const ALL: [BuildSystem; 2] = [BuildSystem::Cargo, BuildSystem::CMake];

    pub fn name(self) -> &'static str {
        match self {
            BuildSystem::Cargo => "Cargo",
            BuildSystem::CMake => "CMake",
        }
    }

    /// The system's identifier in the file.
    fn id(self) -> &'static str {
        match self {
            BuildSystem::Cargo => "cargo",
            BuildSystem::CMake => "cmake",
        }
    }

    fn from_id(id: &str) -> Option<BuildSystem> {
        BuildSystem::ALL
            .into_iter()
            .find(|system| system.id() == id)
    }

    /// The name of the file at a root of this system.
    pub fn file_name(self) -> &'static str {
        match self {
            BuildSystem::Cargo => "Cargo.toml",
            BuildSystem::CMake => "CMakeLists.txt",
        }
    }

    /// The system whose root file `path` is, by its name.
    pub fn of_file(path: &Path) -> Option<BuildSystem> {
        let name = path.file_name()?.to_str()?;
        BuildSystem::ALL
            .into_iter()
            .find(|system| system.file_name() == name)
    }

    /// The configurations a fresh root of this system starts with.
    /// The targets a root of this system has before the project is
    /// looked at, because they cost nothing to know: CMake's `All`.
    fn instant_targets(self) -> Vec<Target> {
        match self {
            BuildSystem::Cargo => Vec::new(),
            BuildSystem::CMake => vec![cmake_all_target()],
        }
    }

    pub fn default_configurations(self) -> Vec<Configuration> {
        match self {
            BuildSystem::Cargo => vec![
                Configuration {
                    name: "dev".to_owned(),
                    profile: "dev".to_owned(),
                    ..Configuration::default()
                },
                Configuration {
                    name: "release".to_owned(),
                    profile: "release".to_owned(),
                    ..Configuration::default()
                },
            ],
            BuildSystem::CMake => vec![
                Configuration {
                    name: "Debug".to_owned(),
                    build_type: "Debug".to_owned(),
                    ..Configuration::default()
                },
                Configuration {
                    name: "Release".to_owned(),
                    build_type: "Release".to_owned(),
                    ..Configuration::default()
                },
            ],
        }
    }

    /// The options a configuration of this system shows, in order.
    pub fn configuration_keys(self) -> &'static [ConfigurationKey] {
        match self {
            BuildSystem::Cargo => &[
                ConfigurationKey::Name,
                ConfigurationKey::Profile,
                ConfigurationKey::BuildArgs,
                ConfigurationKey::Environment,
            ],
            BuildSystem::CMake => &[
                ConfigurationKey::Name,
                ConfigurationKey::BuildType,
                ConfigurationKey::ConfigureArgs,
                ConfigurationKey::BuildArgs,
                ConfigurationKey::Environment,
            ],
        }
    }

    /// The options a target of this system shows, in order.
    pub fn target_keys(self) -> &'static [TargetKey] {
        match self {
            BuildSystem::Cargo => &[
                TargetKey::Name,
                TargetKey::Package,
                TargetKey::Binary,
                TargetKey::WorkingDirectory,
                TargetKey::Arguments,
                TargetKey::Environment,
            ],
            BuildSystem::CMake => &[
                TargetKey::Name,
                TargetKey::CMakeTarget,
                TargetKey::Executable,
                TargetKey::WorkingDirectory,
                TargetKey::Arguments,
                TargetKey::Environment,
            ],
        }
    }
}

// ----- Options --------------------------------------------------------------

/// One option of a configuration. Which ones a configuration shows
/// depends on its root's system; see
/// [`BuildSystem::configuration_keys`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigurationKey {
    Name,
    /// Cargo: the profile to build with.
    Profile,
    /// CMake: `CMAKE_BUILD_TYPE`.
    BuildType,
    /// CMake: extra arguments for the configure step.
    ConfigureArgs,
    /// Extra arguments for the build command.
    BuildArgs,
    /// Environment variables set while building.
    Environment,
}

impl ConfigurationKey {
    /// The short name shown beside the option's field.
    pub fn name(self) -> &'static str {
        match self {
            ConfigurationKey::Name => "Name",
            ConfigurationKey::Profile => "Profile",
            ConfigurationKey::BuildType => "Build type",
            ConfigurationKey::ConfigureArgs => "Configure arguments",
            ConfigurationKey::BuildArgs => "Build arguments",
            ConfigurationKey::Environment => "Environment",
        }
    }

    /// A line about what the option does, for a root of `system`.
    pub fn description(self, system: BuildSystem) -> &'static str {
        match (self, system) {
            (ConfigurationKey::Name, BuildSystem::Cargo) => "What this way of building is called",
            (ConfigurationKey::Name, BuildSystem::CMake) => {
                "What this way of building is called; it builds in cmake-build-<name>, lowercased"
            }
            (ConfigurationKey::Profile, _) => {
                "The Cargo profile to build with: dev, release, or one defined in Cargo.toml"
            }
            (ConfigurationKey::BuildType, _) => {
                "CMAKE_BUILD_TYPE: Debug, Release, RelWithDebInfo, or MinSizeRel; blank to leave unset"
            }
            (ConfigurationKey::ConfigureArgs, _) => {
                "Added to the cmake configure command: -DOPTION=value, -G, and the like"
            }
            (ConfigurationKey::BuildArgs, BuildSystem::Cargo) => {
                "Added to every cargo build and cargo run: --features, -j, and the like"
            }
            (ConfigurationKey::BuildArgs, BuildSystem::CMake) => {
                "Added to cmake --build: -j, --verbose, and the like"
            }
            (ConfigurationKey::Environment, _) => {
                "Variables set while building, as NAME=value pairs separated by spaces"
            }
        }
    }

    /// What the option's field shows while blank.
    pub fn placeholder(self) -> &'static str {
        match self {
            ConfigurationKey::Name => "required",
            ConfigurationKey::Profile => "dev",
            ConfigurationKey::BuildType => "unset",
            ConfigurationKey::ConfigureArgs
            | ConfigurationKey::BuildArgs
            | ConfigurationKey::Environment => "none",
        }
    }

    /// The option's key in the file.
    fn file_key(self) -> &'static str {
        match self {
            ConfigurationKey::Name => "name",
            ConfigurationKey::Profile => "profile",
            ConfigurationKey::BuildType => "build-type",
            ConfigurationKey::ConfigureArgs => "configure-args",
            ConfigurationKey::BuildArgs => "build-args",
            ConfigurationKey::Environment => "env",
        }
    }

    const ALL: [ConfigurationKey; 6] = [
        ConfigurationKey::Name,
        ConfigurationKey::Profile,
        ConfigurationKey::BuildType,
        ConfigurationKey::ConfigureArgs,
        ConfigurationKey::BuildArgs,
        ConfigurationKey::Environment,
    ];
}

/// One option of a target; see [`BuildSystem::target_keys`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKey {
    Name,
    /// Cargo: the package to build and run.
    Package,
    /// Cargo: the binary to run, when the package has several.
    Binary,
    /// CMake: the target to build.
    CMakeTarget,
    /// CMake: the program to run, relative to the build directory.
    Executable,
    /// Where the program runs.
    WorkingDirectory,
    /// The program's command line arguments.
    Arguments,
    /// Environment variables set while the program runs.
    Environment,
}

impl TargetKey {
    pub fn name(self) -> &'static str {
        match self {
            TargetKey::Name => "Name",
            TargetKey::Package => "Package",
            TargetKey::Binary => "Binary",
            TargetKey::CMakeTarget => "CMake target",
            TargetKey::Executable => "Executable",
            TargetKey::WorkingDirectory => "Working directory",
            TargetKey::Arguments => "Arguments",
            TargetKey::Environment => "Environment",
        }
    }

    pub fn description(self, system: BuildSystem) -> &'static str {
        match (self, system) {
            (TargetKey::Name, _) => "What this target is called",
            (TargetKey::Package, _) => {
                "The package to build and run (cargo -p); blank for the whole workspace"
            }
            (TargetKey::Binary, _) => {
                "The binary to run (cargo --bin) when the package has several; blank for its only one"
            }
            (TargetKey::CMakeTarget, _) => {
                "The target for cmake --build; blank builds every target"
            }
            (TargetKey::Executable, _) => {
                "The program Ctrl+R runs, relative to the build directory"
            }
            (TargetKey::WorkingDirectory, BuildSystem::Cargo) => {
                "Where the program runs, relative to the Cargo.toml's directory; blank for that directory"
            }
            (TargetKey::WorkingDirectory, BuildSystem::CMake) => {
                "Where the program runs, relative to the CMakeLists.txt's directory; blank for that directory"
            }
            (TargetKey::Arguments, _) => "Command line arguments the program is run with",
            (TargetKey::Environment, _) => {
                "Variables set while the program runs, as NAME=value pairs separated by spaces"
            }
        }
    }

    pub fn placeholder(self, system: BuildSystem) -> &'static str {
        match (self, system) {
            (TargetKey::Name, _) => "required",
            (TargetKey::Package, _) => "whole workspace",
            (TargetKey::Binary, _) => "the package's binary",
            (TargetKey::CMakeTarget, _) => "all targets",
            (TargetKey::Executable, _) => "nothing to run",
            (TargetKey::WorkingDirectory, BuildSystem::Cargo) => "the Cargo.toml's directory",
            (TargetKey::WorkingDirectory, BuildSystem::CMake) => "the CMakeLists.txt's directory",
            (TargetKey::Arguments, _) | (TargetKey::Environment, _) => "none",
        }
    }

    fn file_key(self) -> &'static str {
        match self {
            TargetKey::Name => "name",
            TargetKey::Package => "package",
            TargetKey::Binary => "binary",
            TargetKey::CMakeTarget => "target",
            TargetKey::Executable => "executable",
            TargetKey::WorkingDirectory => "working-directory",
            TargetKey::Arguments => "args",
            TargetKey::Environment => "env",
        }
    }

    const ALL: [TargetKey; 8] = [
        TargetKey::Name,
        TargetKey::Package,
        TargetKey::Binary,
        TargetKey::CMakeTarget,
        TargetKey::Executable,
        TargetKey::WorkingDirectory,
        TargetKey::Arguments,
        TargetKey::Environment,
    ];
}

// ----- Configurations and targets ------------------------------------------

/// A way of building a root. Every option is kept as the text typed for
/// it; options that don't apply to the root's system stay blank.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Configuration {
    pub name: String,
    pub profile: String,
    pub build_type: String,
    pub configure_args: String,
    pub build_args: String,
    pub environment: String,
}

impl Configuration {
    pub fn text(&self, key: ConfigurationKey) -> &str {
        match key {
            ConfigurationKey::Name => &self.name,
            ConfigurationKey::Profile => &self.profile,
            ConfigurationKey::BuildType => &self.build_type,
            ConfigurationKey::ConfigureArgs => &self.configure_args,
            ConfigurationKey::BuildArgs => &self.build_args,
            ConfigurationKey::Environment => &self.environment,
        }
    }

    fn text_mut(&mut self, key: ConfigurationKey) -> &mut String {
        match key {
            ConfigurationKey::Name => &mut self.name,
            ConfigurationKey::Profile => &mut self.profile,
            ConfigurationKey::BuildType => &mut self.build_type,
            ConfigurationKey::ConfigureArgs => &mut self.configure_args,
            ConfigurationKey::BuildArgs => &mut self.build_args,
            ConfigurationKey::Environment => &mut self.environment,
        }
    }

    /// The name of the directory a CMake configuration builds in:
    /// `cmake-build-<name>`, lowercased.
    pub fn cmake_build_dir_name(&self) -> String {
        format!("{CMAKE_BUILD_DIR_PREFIX}{}", self.name.to_lowercase())
    }

    /// The generator the CMake configure step passes with `-G`: the one
    /// from the settings, unless the configuration's configure arguments
    /// give their own or the setting is blank (leaving the choice to
    /// CMake), in which case `None`.
    pub fn cmake_generator(&self, settings: &Settings) -> Option<String> {
        let generator = settings.cmake_generator();
        if generator.is_empty() {
            return None;
        }
        let args = split_args(&self.configure_args).unwrap_or_default();
        let names_one = args.iter().any(|arg| arg.starts_with("-G"));
        (!names_one).then(|| generator.to_owned())
    }

    /// Everything the CMake configure step depends on, so a frontend can
    /// tell when a build directory needs configuring again.
    pub fn cmake_configure_signature(&self, settings: &Settings) -> String {
        format!(
            "{}\0{}\0{}\0{}",
            self.build_type,
            self.configure_args,
            self.environment,
            self.cmake_generator(settings).unwrap_or_default()
        )
    }
}

/// Something to build and run under a root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub package: String,
    pub binary: String,
    pub cmake_target: String,
    pub executable: String,
    pub working_directory: String,
    pub arguments: String,
    pub environment: String,
    /// For a discovered target, what it was found as (a package name, a
    /// CMake target name), which is how it is matched to the next
    /// discovery whatever it is renamed to. `None` for one the user
    /// made.
    pub auto: Option<String>,
    /// A discovered target the user has put out of the way: kept in the
    /// list, but not built or offered to build.
    pub disabled: bool,
}

impl Target {
    /// Whether the target was found in the project rather than made by
    /// the user.
    pub fn is_auto(&self) -> bool {
        self.auto.is_some()
    }

    /// The options alone, without the origin and the disabled flag, to
    /// compare against a discovered default.
    fn options(&self) -> Target {
        Target {
            auto: None,
            disabled: false,
            ..self.clone()
        }
    }

    pub fn text(&self, key: TargetKey) -> &str {
        match key {
            TargetKey::Name => &self.name,
            TargetKey::Package => &self.package,
            TargetKey::Binary => &self.binary,
            TargetKey::CMakeTarget => &self.cmake_target,
            TargetKey::Executable => &self.executable,
            TargetKey::WorkingDirectory => &self.working_directory,
            TargetKey::Arguments => &self.arguments,
            TargetKey::Environment => &self.environment,
        }
    }

    fn text_mut(&mut self, key: TargetKey) -> &mut String {
        match key {
            TargetKey::Name => &mut self.name,
            TargetKey::Package => &mut self.package,
            TargetKey::Binary => &mut self.binary,
            TargetKey::CMakeTarget => &mut self.cmake_target,
            TargetKey::Executable => &mut self.executable,
            TargetKey::WorkingDirectory => &mut self.working_directory,
            TargetKey::Arguments => &mut self.arguments,
            TargetKey::Environment => &mut self.environment,
        }
    }
}

// ----- Roots ----------------------------------------------------------------

/// A `Cargo.toml` or `CMakeLists.txt` with its configurations and
/// targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRoot {
    system: BuildSystem,
    /// The root file, relative to the project root.
    path: PathBuf,
    configurations: Vec<Configuration>,
    targets: Vec<Target>,
    /// What the last discovery found under this root, as the targets
    /// would be with their default options: what a discovered target
    /// is compared against to tell whether it has been changed. Not
    /// kept in the file; the project is looked at again each load.
    discovered: Vec<Target>,
    /// The origin of the discovered target a fresh root should start
    /// on, when discovery has an opinion (a Cargo package with a
    /// binary); the first target otherwise.
    preferred: Option<String>,
    /// Whether a discovery has been asked for and not yet applied: the
    /// targets are what the file and the seed gave, with more to come.
    pending: bool,
}

impl BuildRoot {
    /// A root at `path` (relative to the project root) with no
    /// configurations or targets yet.
    pub fn new(system: BuildSystem, path: impl Into<PathBuf>) -> BuildRoot {
        BuildRoot {
            system,
            path: path.into(),
            configurations: Vec::new(),
            targets: Vec::new(),
            discovered: Vec::new(),
            preferred: None,
            pending: false,
        }
    }

    /// A root at `path` (relative to the project root) with the system's
    /// default configurations, waiting for its targets to be found: it
    /// has only those that cost nothing to know (a CMake root's `All`)
    /// until a [`Discovery`] for it is run and applied. Returns `None`
    /// if `path` isn't named like a root file.
    pub fn pending(path: impl Into<PathBuf>) -> Option<BuildRoot> {
        let path = path.into();
        let system = BuildSystem::of_file(&path)?;
        let mut root = BuildRoot::new(system, path);
        root.configurations = system.default_configurations();
        root.apply_discovered(Discovered {
            targets: system.instant_targets(),
            preferred: None,
        });
        root.pending = true;
        Some(root)
    }

    /// A root at `path` (relative to `project_root`) with the system's
    /// default configurations and the targets found in the tree: a
    /// Cargo root's packages, or a CMake root's executables, found on
    /// this thread. Returns `None` if `path` isn't named like a root
    /// file.
    pub fn discover(project_root: &Path, path: impl Into<PathBuf>) -> Option<BuildRoot> {
        let mut root = BuildRoot::pending(path)?;
        root.sync_discovered(project_root);
        Some(root)
    }

    /// Whether a discovery has been asked for and hasn't reported yet,
    /// so the targets are not all there.
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// The work of finding this root's targets, to run on any thread;
    /// see [`Discovery`].
    pub fn discovery(&self, project_root: &Path) -> Discovery {
        Discovery {
            path: self.path.clone(),
            system: self.system,
            file: project_root.join(&self.path),
        }
    }

    /// Look at the project again, on this thread, and bring the targets
    /// up to date with what is there; see
    /// [`apply_discovered`](Self::apply_discovered).
    pub fn sync_discovered(&mut self, project_root: &Path) {
        let found = self.discovery(project_root).run(None).found;
        self.apply_discovered(found);
    }

    /// Bring the targets up to date with what discovery found: a newly
    /// found target is added with its defaults, a found one already in
    /// the list is kept as it is (changes, disabling, and all), and one
    /// no longer found goes away if it was never changed or was
    /// disabled, and otherwise stays as an ordinary target with its
    /// changes. Discovered targets come first, in the order they are
    /// found; the user's own follow in their order.
    fn apply_discovered(&mut self, found: Discovered) {
        self.pending = false;
        self.discovered = found.targets;
        self.preferred = found.preferred;
        let mut existing = std::mem::take(&mut self.targets);
        let mut merged: Vec<Target> = Vec::new();
        for default in &self.discovered {
            let found = existing
                .iter()
                .position(|t| t.auto.is_some() && t.auto == default.auto);
            match found {
                Some(index) => merged.push(existing.remove(index)),
                None => {
                    let mut target = default.clone();
                    let taken = |name: &str| {
                        merged
                            .iter()
                            .chain(existing.iter())
                            .any(|t| t.name.eq_ignore_ascii_case(name))
                    };
                    if taken(&target.name) {
                        target.name = free_name(&target.name, |name| !taken(name));
                    }
                    merged.push(target);
                }
            }
        }
        for mut target in existing {
            if target.auto.is_some() {
                // Its source is gone. Disabled, it wasn't wanted; changed,
                // the changes are kept as a target of the user's own.
                if target.disabled {
                    continue;
                }
                target.auto = None;
            }
            merged.push(target);
        }
        self.targets = merged;
    }

    /// The target a root just added should start on: the one discovery
    /// prefers (see the [module documentation](self)), else the first
    /// that isn't disabled, else the first.
    pub fn default_target(&self) -> usize {
        self.preferred
            .as_ref()
            .and_then(|id| {
                self.targets
                    .iter()
                    .position(|t| t.auto.as_ref() == Some(id) && !t.disabled)
            })
            .or_else(|| self.targets.iter().position(|t| !t.disabled))
            .unwrap_or(0)
    }

    /// Whether a discovered target still has the options it was found
    /// with, in which case the file needn't mention it. A target of the
    /// user's own never does.
    pub fn is_discovered_default(&self, target: &Target) -> bool {
        target.auto.is_some()
            && !target.disabled
            && self
                .discovered
                .iter()
                .any(|d| d.auto == target.auto && d.options() == target.options())
    }

    /// What a discovered target was found as, for a note about it: the
    /// package or CMake target, or the whole tree.
    pub fn discovered_as(&self, target: &Target) -> Option<String> {
        let id = target.auto.as_deref()?;
        Some(match (self.system, id) {
            (_, ALL_TARGET_ID) => "everything the root builds".to_owned(),
            (BuildSystem::Cargo, name) => format!("the package {name}"),
            (BuildSystem::CMake, name) => format!("the target {name}"),
        })
    }

    pub fn system(&self) -> BuildSystem {
        self.system
    }

    /// The root file, relative to the project root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The root file's directory, relative to the project root (empty
    /// for a root at the top of the project).
    pub fn directory(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new(""))
    }

    /// The root's name for display: its path.
    pub fn label(&self) -> String {
        slash_path(&self.path)
    }

    pub fn configurations(&self) -> &[Configuration] {
        &self.configurations
    }

    /// The configurations' indices in name order (case-insensitive,
    /// ties in list order), for a frontend listing them.
    pub fn configurations_by_name(&self) -> Vec<usize> {
        sorted_by_name(self.configurations.iter().map(|c| c.name.as_str()))
    }

    /// The targets' indices in name order, likewise.
    pub fn targets_by_name(&self) -> Vec<usize> {
        sorted_by_name(self.targets.iter().map(|t| t.name.as_str()))
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    /// Whether a name is free for a configuration other than the one at
    /// `except`. Names are compared case-insensitively, since a CMake
    /// configuration's name becomes a directory name lowercased.
    fn configuration_name_free(&self, name: &str, except: Option<usize>) -> bool {
        !self
            .configurations
            .iter()
            .enumerate()
            .any(|(i, c)| Some(i) != except && c.name.eq_ignore_ascii_case(name))
    }

    fn target_name_free(&self, name: &str, except: Option<usize>) -> bool {
        !self
            .targets
            .iter()
            .enumerate()
            .any(|(i, t)| Some(i) != except && t.name.eq_ignore_ascii_case(name))
    }

    /// A name not yet used by a configuration: `base`, or `base 2`,
    /// `base 3`, ...
    fn free_configuration_name(&self, base: &str) -> String {
        free_name(base, |name| self.configuration_name_free(name, None))
    }

    fn free_target_name(&self, base: &str) -> String {
        free_name(base, |name| self.target_name_free(name, None))
    }
}

/// The indices of `names` in case-insensitive order, equal names in
/// their own order.
fn sorted_by_name<'a>(names: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let mut indices: Vec<(String, usize)> = names
        .enumerate()
        .map(|(i, name)| (name.to_lowercase(), i))
        .collect();
    indices.sort();
    indices.into_iter().map(|(_, i)| i).collect()
}

fn free_name(base: &str, is_free: impl Fn(&str) -> bool) -> String {
    if is_free(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| is_free(name))
        .expect("some number is free")
}

/// Check a configuration's or target's name as typed.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("enter a name".to_owned());
    }
    if name.contains(['/', '\\']) {
        return Err("a name can't contain / or \\".to_owned());
    }
    Ok(())
}

// ----- The configuration ----------------------------------------------------

/// The current configuration and target: indices into a root's lists.
/// An index past the end of its list (a root with nothing in the list)
/// selects nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub root: usize,
    pub configuration: usize,
    pub target: usize,
}

impl PartialEq for BuildConfig {
    fn eq(&self, other: &BuildConfig) -> bool {
        self.roots == other.roots && self.current == other.current
    }
}

impl Eq for BuildConfig {}

/// Why a build configuration file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildError(pub(crate) String);

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BuildError {}

/// A project's build configuration; see the [module documentation](self).
/// Two configurations are equal when their roots and selection are; the
/// name waiting for a sync doesn't count.
#[derive(Clone, Debug, Default)]
pub struct BuildConfig {
    roots: Vec<BuildRoot>,
    /// The current configuration and target; `None` only with no roots.
    current: Option<Selection>,
    /// The current target's name as the file had it, until
    /// [`sync_discovered`](Self::sync_discovered) can find it among the
    /// discovered targets the file leaves out.
    pending_target: Option<String>,
}

impl BuildConfig {
    /// The configuration a project starts with: a root for the
    /// `Cargo.toml` and the `CMakeLists.txt` at the top of the project,
    /// whichever exist, with their defaults and their targets pending
    /// discovery (see [`request_discovery`](Self::request_discovery)).
    pub fn find_roots(project_root: &Path) -> BuildConfig {
        let mut config = BuildConfig::default();
        for system in BuildSystem::ALL {
            if project_root.join(system.file_name()).is_file()
                && let Some(root) = BuildRoot::pending(system.file_name())
            {
                let _ = config.add_root(root);
            }
        }
        config
    }

    /// [`find_roots`](Self::find_roots) with the targets found on this
    /// thread.
    pub fn discover(project_root: &Path) -> BuildConfig {
        let mut config = BuildConfig::find_roots(project_root);
        config.sync_discovered(project_root);
        config
    }

    pub fn roots(&self) -> &[BuildRoot] {
        &self.roots
    }

    pub fn root(&self, index: usize) -> Option<&BuildRoot> {
        self.roots.get(index)
    }

    // ----- The current selection ------------------------------------------

    pub fn current(&self) -> Option<Selection> {
        self.current
    }

    /// The current configuration with its root, if there is one.
    pub fn current_configuration(&self) -> Option<(&BuildRoot, &Configuration)> {
        self.configuration_for(self.current?)
    }

    /// A selection's configuration with its root, if the selection
    /// points at one.
    pub fn configuration_for(&self, selection: Selection) -> Option<(&BuildRoot, &Configuration)> {
        let root = self.roots.get(selection.root)?;
        Some((root, root.configurations.get(selection.configuration)?))
    }

    /// The current target with its root, if there is one.
    pub fn current_target(&self) -> Option<(&BuildRoot, &Target)> {
        let current = self.current?;
        let root = self.roots.get(current.root)?;
        Some((root, root.targets.get(current.target)?))
    }

    /// Make a configuration the current one. From another root than the
    /// current target's, the target becomes that root's target of the
    /// same name, or its first. Returns whether anything changed.
    pub fn select_configuration(&mut self, root: usize, index: usize) -> bool {
        let Some(new_root) = self.roots.get(root) else {
            return false;
        };
        let before = self.current;
        let target = match self.current {
            Some(current) if current.root == root => current.target,
            Some(current) => {
                let name = self
                    .roots
                    .get(current.root)
                    .and_then(|r| r.targets.get(current.target))
                    .map(|t| t.name.clone());
                // Whatever name was waited for belonged to the old root.
                self.pending_target = None;
                name.and_then(|name| new_root.targets.iter().position(|t| t.name == name))
                    .unwrap_or(0)
            }
            None => 0,
        };
        self.current = Some(Selection {
            root,
            configuration: index,
            target,
        });
        self.current != before
    }

    /// Make a target the current one, bringing the configuration along
    /// as [`select_configuration`](Self::select_configuration) does the
    /// target.
    pub fn select_target(&mut self, root: usize, index: usize) -> bool {
        let Some(selection) = self.selection_with_target(root, index) else {
            return false;
        };
        let before = self.current;
        self.current = Some(selection);
        // A choice made now outranks the name the file was waiting for.
        self.pending_target = None;
        self.current != before
    }

    /// What [`select_target`](Self::select_target) would make current,
    /// without making it so: the target with the current configuration
    /// if it is in the same root, and otherwise with that root's
    /// configuration of the same name, or its first. For running a
    /// target that isn't the current one with what it would run with if
    /// it were. `None` for a root that doesn't exist.
    pub fn selection_with_target(&self, root: usize, index: usize) -> Option<Selection> {
        let new_root = self.roots.get(root)?;
        let configuration = match self.current {
            Some(current) if current.root == root => current.configuration,
            Some(current) => {
                let name = self
                    .roots
                    .get(current.root)
                    .and_then(|r| r.configurations.get(current.configuration))
                    .map(|c| c.name.as_str());
                name.and_then(|name| new_root.configurations.iter().position(|c| c.name == name))
                    .unwrap_or(0)
            }
            None => 0,
        };
        Some(Selection {
            root,
            configuration,
            target: index,
        })
    }

    /// Keep the selection pointing at something after the lists changed:
    /// the first root when there is none, and indices within their lists
    /// where the lists aren't empty.
    fn settle_selection(&mut self) {
        if self.roots.is_empty() {
            self.current = None;
            return;
        }
        let mut current = self.current.unwrap_or(Selection {
            root: 0,
            configuration: 0,
            target: 0,
        });
        if current.root >= self.roots.len() {
            current = Selection {
                root: 0,
                configuration: 0,
                target: 0,
            };
        }
        let root = &self.roots[current.root];
        if !root.configurations.is_empty() {
            current.configuration = current.configuration.min(root.configurations.len() - 1);
        }
        if !root.targets.is_empty() {
            current.target = current.target.min(root.targets.len() - 1);
        }
        self.current = Some(current);
    }

    // ----- Editing the lists ----------------------------------------------

    /// Add a root, unless one at the same path is already there. Returns
    /// its index. The first root added becomes the current one, on the
    /// target discovery prefers.
    pub fn add_root(&mut self, root: BuildRoot) -> Result<usize, String> {
        if self.roots.iter().any(|r| r.path == root.path) {
            return Err(format!("{} is already a build root", root.label()));
        }
        let target = root.default_target();
        self.roots.push(root);
        let index = self.roots.len() - 1;
        if self.current.is_none() {
            self.current = Some(Selection {
                root: index,
                configuration: 0,
                target,
            });
        }
        self.settle_selection();
        Ok(index)
    }

    pub fn remove_root(&mut self, index: usize) {
        if index >= self.roots.len() {
            return;
        }
        self.roots.remove(index);
        if let Some(current) = &mut self.current {
            if current.root == index {
                self.current = None;
            } else if current.root > index {
                current.root -= 1;
            }
        }
        self.settle_selection();
    }

    /// Add a blank configuration to a root, named `New configuration`
    /// (numbered if that is taken). Returns its index.
    pub fn add_configuration(&mut self, root: usize) -> Option<usize> {
        let r = self.roots.get_mut(root)?;
        let name = r.free_configuration_name("New configuration");
        r.configurations.push(Configuration {
            name,
            ..Configuration::default()
        });
        let index = r.configurations.len() - 1;
        self.settle_selection();
        Some(index)
    }

    /// Add a copy of a configuration after it, named `<name> copy`.
    /// Returns the copy's index.
    pub fn duplicate_configuration(&mut self, root: usize, index: usize) -> Option<usize> {
        let r = self.roots.get_mut(root)?;
        let original = r.configurations.get(index)?.clone();
        let name = r.free_configuration_name(&format!("{} copy", original.name));
        r.configurations
            .insert(index + 1, Configuration { name, ..original });
        if let Some(current) = &mut self.current
            && current.root == root
            && current.configuration > index
        {
            current.configuration += 1;
        }
        Some(index + 1)
    }

    pub fn remove_configuration(&mut self, root: usize, index: usize) {
        let Some(r) = self.roots.get_mut(root) else {
            return;
        };
        if index >= r.configurations.len() {
            return;
        }
        r.configurations.remove(index);
        if let Some(current) = &mut self.current
            && current.root == root
            && current.configuration > index
        {
            current.configuration -= 1;
        }
        self.settle_selection();
    }

    pub fn add_target(&mut self, root: usize) -> Option<usize> {
        let r = self.roots.get_mut(root)?;
        let name = r.free_target_name("New target");
        r.targets.push(Target {
            name,
            ..Target::default()
        });
        let index = r.targets.len() - 1;
        self.settle_selection();
        Some(index)
    }

    /// Add a copy of a target after it, as a target of the user's own
    /// whatever the original was.
    pub fn duplicate_target(&mut self, root: usize, index: usize) -> Option<usize> {
        let r = self.roots.get_mut(root)?;
        let original = r.targets.get(index)?.clone();
        let name = r.free_target_name(&format!("{} copy", original.name));
        r.targets.insert(
            index + 1,
            Target {
                name,
                auto: None,
                disabled: false,
                ..original
            },
        );
        if let Some(current) = &mut self.current
            && current.root == root
            && current.target > index
        {
            current.target += 1;
        }
        Some(index + 1)
    }

    /// Remove a target of the user's own. A discovered target can only
    /// be disabled, since removing it would only have discovery bring
    /// it back; the error says so.
    pub fn remove_target(&mut self, root: usize, index: usize) -> Result<(), String> {
        let Some(r) = self.roots.get_mut(root) else {
            return Ok(());
        };
        let Some(target) = r.targets.get(index) else {
            return Ok(());
        };
        if target.is_auto() {
            return Err(format!(
                "{} was found in {}, so it can only be disabled, not removed",
                target.name,
                r.label()
            ));
        }
        r.targets.remove(index);
        if let Some(current) = &mut self.current
            && current.root == root
            && current.target > index
        {
            current.target -= 1;
        }
        self.settle_selection();
        Ok(())
    }

    /// Disable or enable a target. Disabling the current target moves
    /// the selection to the root's first enabled one, if it has one.
    /// Returns whether anything changed.
    pub fn set_target_disabled(&mut self, root: usize, index: usize, disabled: bool) -> bool {
        let Some(r) = self.roots.get_mut(root) else {
            return false;
        };
        let Some(target) = r.targets.get_mut(index) else {
            return false;
        };
        if target.disabled == disabled {
            return false;
        }
        target.disabled = disabled;
        if disabled
            && let Some(current) = &mut self.current
            && current.root == root
            && current.target == index
            && let Some(enabled) = r.targets.iter().position(|t| !t.disabled)
        {
            current.target = enabled;
        }
        true
    }

    // ----- Options as text ------------------------------------------------

    /// A configuration option's text: what its field holds.
    pub fn configuration_text(&self, root: usize, index: usize, key: ConfigurationKey) -> String {
        self.roots
            .get(root)
            .and_then(|r| r.configurations.get(index))
            .map(|c| c.text(key).to_owned())
            .unwrap_or_default()
    }

    /// Set a configuration option from text, as typed into a field.
    /// Surrounding spaces are dropped. Returns whether the option
    /// changed, or why the text was refused: a blank or duplicate name,
    /// arguments with an unclosed quote, an environment entry without
    /// its `=`.
    pub fn set_configuration_text(
        &mut self,
        root: usize,
        index: usize,
        key: ConfigurationKey,
        text: &str,
    ) -> Result<bool, String> {
        let text = text.trim();
        let r = self.roots.get_mut(root).ok_or("no such root")?;
        match key {
            ConfigurationKey::Name => {
                check_name(text)?;
                if !r.configuration_name_free(text, Some(index)) {
                    return Err(format!("there is already a configuration named {text}"));
                }
            }
            ConfigurationKey::ConfigureArgs | ConfigurationKey::BuildArgs => {
                split_args(text)?;
            }
            ConfigurationKey::Environment => {
                parse_environment(text)?;
            }
            ConfigurationKey::Profile | ConfigurationKey::BuildType => {}
        }
        let configuration = r
            .configurations
            .get_mut(index)
            .ok_or("no such configuration")?;
        let field = configuration.text_mut(key);
        if field == text {
            return Ok(false);
        }
        *field = text.to_owned();
        Ok(true)
    }

    pub fn target_text(&self, root: usize, index: usize, key: TargetKey) -> String {
        self.roots
            .get(root)
            .and_then(|r| r.targets.get(index))
            .map(|t| t.text(key).to_owned())
            .unwrap_or_default()
    }

    /// Set a target option from text; see
    /// [`set_configuration_text`](Self::set_configuration_text).
    pub fn set_target_text(
        &mut self,
        root: usize,
        index: usize,
        key: TargetKey,
        text: &str,
    ) -> Result<bool, String> {
        let text = text.trim();
        let r = self.roots.get_mut(root).ok_or("no such root")?;
        match key {
            TargetKey::Name => {
                check_name(text)?;
                if !r.target_name_free(text, Some(index)) {
                    return Err(format!("there is already a target named {text}"));
                }
            }
            TargetKey::Arguments => {
                split_args(text)?;
            }
            TargetKey::Environment => {
                parse_environment(text)?;
            }
            TargetKey::Package
            | TargetKey::Binary
            | TargetKey::CMakeTarget
            | TargetKey::Executable
            | TargetKey::WorkingDirectory => {}
        }
        let target = r.targets.get_mut(index).ok_or("no such target")?;
        let field = target.text_mut(key);
        if field == text {
            return Ok(false);
        }
        *field = text.to_owned();
        Ok(true)
    }

    // ----- Building and running -------------------------------------------

    /// The current selection, or why there is none.
    fn current_selection(&self) -> Result<Selection, String> {
        self.current.ok_or_else(|| NO_ROOT_MESSAGE.to_owned())
    }

    /// The current selection as a build or run needs it: `Err` with no
    /// roots, and while discovery is still to find the target the file
    /// named as current, since the one standing in for it until then
    /// would build the wrong thing.
    pub fn current_for_job(&self) -> Result<Selection, String> {
        let selection = self.current_selection()?;
        if let Some(name) = &self.pending_target
            && let Some(root) = self.roots.get(selection.root)
            && root.pending
        {
            return Err(format!(
                "Still finding the targets of {}: {name} hasn't turned up yet",
                root.label()
            ));
        }
        Ok(selection)
    }

    /// A selection's root, configuration, and target, or why there
    /// aren't all three.
    fn parts(&self, selection: Selection) -> Result<(&BuildRoot, &Configuration, &Target), String> {
        let root = self
            .roots
            .get(selection.root)
            .ok_or("No build root selected")?;
        let configuration = root
            .configurations
            .get(selection.configuration)
            .ok_or_else(|| format!("{} has no configuration to build with", root.label()))?;
        let target = root.targets.get(selection.target).ok_or_else(|| {
            if root.pending {
                format!("Still finding the targets of {}", root.label())
            } else {
                format!("{} has no target to build", root.label())
            }
        })?;
        if target.disabled {
            return Err(format!(
                "{} is disabled: enable it on the build configuration page, or pick another target",
                target.name
            ));
        }
        Ok((root, configuration, target))
    }

    /// The directory the current CMake configuration builds in, if the
    /// current root is a CMake one.
    pub fn cmake_build_dir(&self, project_root: &Path) -> Option<PathBuf> {
        self.cmake_build_dir_for(project_root, self.current?)
    }

    /// The directory a selection's CMake configuration builds in, if its
    /// root is a CMake one.
    pub fn cmake_build_dir_for(
        &self,
        project_root: &Path,
        selection: Selection,
    ) -> Option<PathBuf> {
        let (root, configuration) = self.configuration_for(selection)?;
        (root.system == BuildSystem::CMake)
            .then(|| cmake_build_dir(project_root, root, configuration))
    }

    /// The commands that build the current target with the current
    /// configuration. For CMake, `configure` says whether to run the
    /// configure step before the build; a frontend passes `true` until
    /// the build directory has been configured with the configuration
    /// as it now is. The settings supply what isn't the configuration's
    /// to say: the CMake generator.
    pub fn build_job(
        &self,
        project_root: &Path,
        configure: bool,
        settings: &Settings,
    ) -> Result<Job, String> {
        self.build_job_for(self.current_for_job()?, project_root, configure, settings)
    }

    /// The commands that build a selection's target with its
    /// configuration, as [`build_job`](Self::build_job) does the current
    /// one's.
    pub fn build_job_for(
        &self,
        selection: Selection,
        project_root: &Path,
        configure: bool,
        settings: &Settings,
    ) -> Result<Job, String> {
        let (root, configuration, target) = self.parts(selection)?;
        let mut job = Job {
            title: format!("Build {} ({})", target.name, configuration.name),
            steps: Vec::new(),
        };
        match root.system {
            BuildSystem::Cargo => {
                let mut command = cargo_command(project_root, root, configuration, "build")?;
                for arg in cargo_target_args(target) {
                    command = command.arg(arg);
                }
                for arg in split_args(&configuration.build_args)? {
                    command = command.arg(arg);
                }
                job.steps.push(Step {
                    description: format!("Building {}", target.name),
                    command,
                });
            }
            BuildSystem::CMake => {
                let build_dir = cmake_build_dir(project_root, root, configuration);
                if configure {
                    job.steps.push(Step {
                        description: format!("Configuring {}", configuration.name),
                        command: cmake_configure_command(
                            project_root,
                            root,
                            configuration,
                            &build_dir,
                            settings,
                        )?,
                    });
                }
                let mut command = Command::new("cmake")
                    .arg("--build")
                    .arg(&build_dir)
                    .current_dir(root_dir(project_root, root));
                if !target.cmake_target.is_empty() {
                    command = command.arg("--target").arg(&target.cmake_target);
                }
                for arg in split_args(&configuration.build_args)? {
                    command = command.arg(arg);
                }
                command = with_environment(command, &configuration.environment)?;
                job.steps.push(Step {
                    description: format!("Building {}", target.name),
                    command,
                });
            }
        }
        Ok(job)
    }

    /// The commands that build and then run the current target: for
    /// Cargo one `cargo run`, for CMake the build followed by the
    /// target's executable.
    pub fn run_job(
        &self,
        project_root: &Path,
        configure: bool,
        settings: &Settings,
    ) -> Result<Job, String> {
        self.run_job_for(self.current_for_job()?, project_root, configure, settings)
    }

    /// The commands that build and then run a selection's target with
    /// its configuration, as [`run_job`](Self::run_job) does the current
    /// one's. With [`selection_with_target`](Self::selection_with_target)
    /// this runs a target other than the current one, with what it
    /// would run with if it were, without changing what is current.
    pub fn run_job_for(
        &self,
        selection: Selection,
        project_root: &Path,
        configure: bool,
        settings: &Settings,
    ) -> Result<Job, String> {
        let (root, configuration, target) = self.parts(selection)?;
        let dir = root_dir(project_root, root);
        match root.system {
            BuildSystem::Cargo => {
                let mut command = cargo_command(project_root, root, configuration, "run")?
                    .arg("--manifest-path")
                    .arg(project_root.join(&root.path));
                for arg in cargo_target_args(target) {
                    command = command.arg(arg);
                }
                for arg in split_args(&configuration.build_args)? {
                    command = command.arg(arg);
                }
                let arguments = split_args(&target.arguments)?;
                if !arguments.is_empty() {
                    command = command.arg("--");
                    for arg in arguments {
                        command = command.arg(arg);
                    }
                }
                command = with_environment(command, &target.environment)?;
                let working_dir = if target.working_directory.is_empty() {
                    dir
                } else {
                    dir.join(&target.working_directory)
                };
                Ok(Job {
                    title: format!("Run {} ({})", target.name, configuration.name),
                    steps: vec![Step {
                        description: format!("Running {}", target.name),
                        command: command.current_dir(working_dir),
                    }],
                })
            }
            BuildSystem::CMake => {
                if target.executable.is_empty() {
                    return Err(format!(
                        "{} has no executable to run: set one on the build configuration page",
                        target.name
                    ));
                }
                let mut job = self.build_job_for(selection, project_root, configure, settings)?;
                job.title = format!("Run {} ({})", target.name, configuration.name);
                let build_dir = cmake_build_dir(project_root, root, configuration);
                let executable = build_dir.join(&target.executable);
                let working_dir = if target.working_directory.is_empty() {
                    dir
                } else {
                    dir.join(&target.working_directory)
                };
                let mut command = Command::new(&executable).current_dir(working_dir);
                for arg in split_args(&target.arguments)? {
                    command = command.arg(arg);
                }
                command = with_environment(command, &configuration.environment)?;
                command = with_environment(command, &target.environment)?;
                job.steps.push(Step {
                    description: format!("Running {}", target.name),
                    command,
                });
                Ok(job)
            }
        }
    }

    /// The command that deletes what the current configuration has
    /// built, so the next build starts from nothing: for CMake, its
    /// build directory; for Cargo, the profile's artifacts, which
    /// `cargo clean` knows better than this module where to find. Meant
    /// for a command palette rather than a key: it is for debugging a
    /// build, not for every day.
    pub fn clean_job(&self, project_root: &Path) -> Result<Job, String> {
        let (root, configuration) = self
            .current_configuration()
            .ok_or("No build configuration selected")?;
        Ok(Job {
            title: format!("Delete build directory ({})", configuration.name),
            steps: vec![clean_step(project_root, root, configuration)?],
        })
    }

    /// The commands that delete everything every root has built: each
    /// CMake configuration's build directory, and each Cargo root's
    /// whole target directory.
    pub fn clean_all_job(&self, project_root: &Path) -> Result<Job, String> {
        let mut steps = Vec::new();
        for root in &self.roots {
            match root.system {
                BuildSystem::Cargo => {
                    let command = Command::new("cargo")
                        .arg("clean")
                        .arg("--manifest-path")
                        .arg(project_root.join(&root.path))
                        .current_dir(root_dir(project_root, root));
                    steps.push(Step {
                        description: format!("Cleaning {}", root.label()),
                        command,
                    });
                }
                BuildSystem::CMake => {
                    for configuration in &root.configurations {
                        steps.push(clean_step(project_root, root, configuration)?);
                    }
                }
            }
        }
        if steps.is_empty() {
            return Err("No build roots to clean".to_owned());
        }
        Ok(Job {
            title: "Delete all build directories".to_owned(),
            steps,
        })
    }

    // ----- The file -------------------------------------------------------

    /// Look at the project again for every root's discovered targets;
    /// see [`BuildRoot::sync_discovered`]. Done after reading the file,
    /// which holds only what the user changed. The current target stays
    /// the same target, by name, through any reordering.
    pub fn sync_discovered(&mut self, project_root: &Path) {
        for discovery in self.request_discovery(project_root) {
            self.apply_discovery(discovery.run(None));
        }
    }

    /// Ask for every root's targets to be found again: each root is
    /// pending from here until its result comes back through
    /// [`apply_discovery`](Self::apply_discovery). The work is returned
    /// to be run wherever suits, one piece per root, in root order.
    pub fn request_discovery(&mut self, project_root: &Path) -> Vec<Discovery> {
        self.roots
            .iter_mut()
            .map(|root| {
                root.pending = true;
                root.discovery(project_root)
            })
            .collect()
    }

    /// Whether any root is still waiting for its targets to be found.
    pub fn is_discovering(&self) -> bool {
        self.roots.iter().any(|root| root.pending)
    }

    /// The current target's name as the file had it, while discovery is
    /// still to find it: what a frontend should show as current in the
    /// meantime, since the target standing in until then is arbitrary.
    pub fn pending_target(&self) -> Option<&str> {
        let current = self.current?;
        let root = self.roots.get(current.root)?;
        root.pending
            .then_some(self.pending_target.as_deref())
            .flatten()
    }

    /// The index of the root at a path (relative to the project).
    pub fn root_at(&self, path: &Path) -> Option<usize> {
        self.roots.iter().position(|root| root.path == path)
    }

    /// Take in what a discovery found, bringing its root's targets up to
    /// date (see [`BuildRoot::sync_discovered`]). The current target
    /// stays the same target by name, or becomes the one the file named
    /// if that has now turned up; a root that had no target yet starts
    /// on the one discovery prefers. Returns the root's index, or
    /// `None` for a root that has since been removed.
    pub fn apply_discovery(&mut self, result: DiscoveryResult) -> Option<usize> {
        let index = self.root_at(&result.path)?;
        let current_name = match self.current {
            Some(current) if current.root == index => self
                .pending_target
                .take()
                .or_else(|| self.current_target().map(|(_, t)| t.name.clone())),
            _ => None,
        };
        let root = &mut self.roots[index];
        let had_target = root
            .targets
            .get(self.current.map_or(0, |c| c.target))
            .is_some();
        root.apply_discovered(result.found);
        if let Some(current) = &mut self.current
            && current.root == index
        {
            match current_name {
                Some(name) => {
                    if let Some(target) = root.targets.iter().position(|t| t.name == name) {
                        current.target = target;
                    }
                }
                None if !had_target => current.target = root.default_target(),
                None => {}
            }
        }
        self.settle_selection();
        Some(index)
    }

    /// Read a build configuration file. Keys the editor doesn't know are
    /// ignored; a value of the wrong type is an error. The discovered
    /// targets the file leaves out come from
    /// [`sync_discovered`](Self::sync_discovered), to be called after.
    pub fn parse(text: &str) -> Result<BuildConfig, BuildError> {
        let table: Table = text
            .parse()
            .map_err(|err: toml::de::Error| BuildError(err.message().to_owned()))?;
        let mut config = BuildConfig::default();
        if let Some(roots) = table.get("roots") {
            let roots = roots
                .as_array()
                .ok_or_else(|| BuildError("`roots` must be an array of tables".to_owned()))?;
            for (i, root) in roots.iter().enumerate() {
                let root = root
                    .as_table()
                    .ok_or_else(|| BuildError(format!("`roots[{i}]` must be a table")))?;
                config.roots.push(parse_root(root, i)?);
            }
        }
        // The current selection, by name; anything missing falls back to
        // the first of its kind.
        let current = table.get("current").and_then(Value::as_table);
        let name = |key: &str| current.and_then(|t| t.get(key)).and_then(Value::as_str);
        let root = name("root")
            .and_then(|path| config.roots.iter().position(|r| r.label() == path))
            .unwrap_or(0);
        let configuration = name("configuration")
            .and_then(|n| {
                config
                    .roots
                    .get(root)?
                    .configurations
                    .iter()
                    .position(|c| c.name == n)
            })
            .unwrap_or(0);
        let target = name("target")
            .and_then(|n| {
                config
                    .roots
                    .get(root)?
                    .targets
                    .iter()
                    .position(|t| t.name == n)
            })
            .unwrap_or(0);
        config.pending_target = name("target").map(str::to_owned);
        config.current = Some(Selection {
            root,
            configuration,
            target,
        });
        config.settle_selection();
        Ok(config)
    }

    /// The configuration as a file. Options left blank are left out, and
    /// so are discovered targets with their default options, which the
    /// next load finds again.
    pub fn to_toml(&self) -> String {
        let mut table = Table::new();
        if let Some(current) = self.current
            && let Some(root) = self.roots.get(current.root)
        {
            let mut t = Table::new();
            t.insert("root".to_owned(), Value::String(root.label()));
            if let Some(c) = root.configurations.get(current.configuration) {
                t.insert("configuration".to_owned(), Value::String(c.name.clone()));
            }
            // While discovery is still to find the target the file
            // named, that name stands rather than the stand-in's.
            let target = self
                .pending_target
                .clone()
                .or_else(|| root.targets.get(current.target).map(|t| t.name.clone()));
            if let Some(target) = target {
                t.insert("target".to_owned(), Value::String(target));
            }
            table.insert("current".to_owned(), Value::Table(t));
        }
        let roots: Vec<Value> = self
            .roots
            .iter()
            .map(|root| {
                let mut t = Table::new();
                t.insert(
                    "system".to_owned(),
                    Value::String(root.system.id().to_owned()),
                );
                t.insert("path".to_owned(), Value::String(root.label()));
                let configurations: Vec<Value> = root
                    .configurations
                    .iter()
                    .map(|c| {
                        let mut t = Table::new();
                        for key in ConfigurationKey::ALL {
                            let text = c.text(key);
                            if !text.is_empty() {
                                t.insert(key.file_key().to_owned(), Value::String(text.to_owned()));
                            }
                        }
                        Value::Table(t)
                    })
                    .collect();
                t.insert("configurations".to_owned(), Value::Array(configurations));
                let targets: Vec<Value> = root
                    .targets
                    .iter()
                    .filter(|target| !root.is_discovered_default(target))
                    .map(|target| {
                        let mut t = Table::new();
                        if let Some(auto) = &target.auto {
                            t.insert(AUTO_KEY.to_owned(), Value::String(auto.clone()));
                        }
                        if target.disabled {
                            t.insert(DISABLED_KEY.to_owned(), Value::Boolean(true));
                        }
                        for key in TargetKey::ALL {
                            let text = target.text(key);
                            if !text.is_empty() {
                                t.insert(key.file_key().to_owned(), Value::String(text.to_owned()));
                            }
                        }
                        Value::Table(t)
                    })
                    .collect();
                t.insert("targets".to_owned(), Value::Array(targets));
                Value::Table(t)
            })
            .collect();
        table.insert("roots".to_owned(), Value::Array(roots));
        format!("{FILE_HEADER}\n{table}")
    }
}

fn parse_root(table: &Table, index: usize) -> Result<BuildRoot, BuildError> {
    let string = |t: &Table, key: &str, what: &str| -> Result<Option<String>, BuildError> {
        match t.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(BuildError(format!("`{what}.{key}` must be a string"))),
        }
    };
    let what = format!("roots[{index}]");
    let system = string(table, "system", &what)?
        .ok_or_else(|| BuildError(format!("`{what}` has no `system`")))?;
    let system = BuildSystem::from_id(&system)
        .ok_or_else(|| BuildError(format!("`{what}.system` must be cargo or cmake")))?;
    let path = string(table, "path", &what)?
        .ok_or_else(|| BuildError(format!("`{what}` has no `path`")))?;
    let mut root = BuildRoot::new(system, PathBuf::from(path));
    let list = |key: &str| -> Result<Vec<&Table>, BuildError> {
        match table.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_table().ok_or_else(|| {
                        BuildError(format!("`{what}.{key}` must be an array of tables"))
                    })
                })
                .collect(),
            Some(_) => Err(BuildError(format!(
                "`{what}.{key}` must be an array of tables"
            ))),
        }
    };
    for (i, t) in list("configurations")?.into_iter().enumerate() {
        let what = format!("{what}.configurations[{i}]");
        let mut configuration = Configuration::default();
        for key in ConfigurationKey::ALL {
            if let Some(text) = string(t, key.file_key(), &what)? {
                *configuration.text_mut(key) = text.trim().to_owned();
            }
        }
        if configuration.name.is_empty() {
            configuration.name = root.free_configuration_name("Unnamed");
        }
        root.configurations.push(configuration);
    }
    for (i, t) in list("targets")?.into_iter().enumerate() {
        let what = format!("{what}.targets[{i}]");
        let mut target = Target::default();
        for key in TargetKey::ALL {
            if let Some(text) = string(t, key.file_key(), &what)? {
                *target.text_mut(key) = text.trim().to_owned();
            }
        }
        target.auto = string(t, AUTO_KEY, &what)?;
        target.disabled = match t.get(DISABLED_KEY) {
            None => false,
            Some(Value::Boolean(disabled)) => *disabled,
            Some(_) => {
                return Err(BuildError(format!(
                    "`{what}.{DISABLED_KEY}` must be true or false"
                )));
            }
        };
        if target.name.is_empty() {
            target.name = root.free_target_name("Unnamed");
        }
        root.targets.push(target);
    }
    Ok(root)
}

/// A relative path with forward slashes, for the file and for display.
fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

// ----- Jobs -----------------------------------------------------------------

/// One command of a job.
#[derive(Clone, Debug)]
pub struct Step {
    /// What the step is doing, for a heading above its output.
    pub description: String,
    pub command: Command,
}

/// The commands a build or run consists of, to run one after another,
/// stopping at the first that fails.
#[derive(Clone, Debug)]
pub struct Job {
    /// What the job is, for a tab or a status line.
    pub title: String,
    pub steps: Vec<Step>,
}

/// The directory a root's file is in, absolute.
fn root_dir(project_root: &Path, root: &BuildRoot) -> PathBuf {
    project_root.join(root.directory())
}

fn cmake_build_dir(
    project_root: &Path,
    root: &BuildRoot,
    configuration: &Configuration,
) -> PathBuf {
    root_dir(project_root, root).join(configuration.cmake_build_dir_name())
}

/// `cargo <subcommand>` in the root's directory with the configuration's
/// profile and environment.
/// The step that deletes what `configuration` has built under `root`.
/// CMake's own `-E rm` does the deleting, so the step is the same on
/// every platform and its failure shows in the output like any other
/// command's.
fn clean_step(
    project_root: &Path,
    root: &BuildRoot,
    configuration: &Configuration,
) -> Result<Step, String> {
    match root.system {
        BuildSystem::Cargo => {
            let command = cargo_command(project_root, root, configuration, "clean")?
                .arg("--manifest-path")
                .arg(project_root.join(&root.path));
            Ok(Step {
                description: format!("Cleaning {} ({})", root.label(), configuration.name),
                command,
            })
        }
        BuildSystem::CMake => {
            let build_dir = cmake_build_dir(project_root, root, configuration);
            let command = Command::new("cmake")
                .arg("-E")
                .arg("rm")
                .arg("-rf")
                .arg(&build_dir)
                .current_dir(root_dir(project_root, root));
            Ok(Step {
                description: format!("Deleting {}", build_dir.display()),
                command,
            })
        }
    }
}

fn cargo_command(
    project_root: &Path,
    root: &BuildRoot,
    configuration: &Configuration,
    subcommand: &str,
) -> Result<Command, String> {
    let mut command = Command::new("cargo")
        .arg(subcommand)
        .current_dir(root_dir(project_root, root));
    match configuration.profile.as_str() {
        "" | "dev" => {}
        "release" => command = command.arg("--release"),
        profile => command = command.arg("--profile").arg(profile),
    }
    with_environment(command, &configuration.environment)
}

/// The arguments that pick a target's package and binary.
fn cargo_target_args(target: &Target) -> Vec<String> {
    let mut args = Vec::new();
    if !target.package.is_empty() {
        args.push("-p".to_owned());
        args.push(target.package.clone());
    }
    if !target.binary.is_empty() {
        args.push("--bin".to_owned());
        args.push(target.binary.clone());
    }
    args
}

fn cmake_configure_command(
    project_root: &Path,
    root: &BuildRoot,
    configuration: &Configuration,
    build_dir: &Path,
    settings: &Settings,
) -> Result<Command, String> {
    let dir = root_dir(project_root, root);
    let mut command = Command::new("cmake")
        .arg("-S")
        .arg(&dir)
        .arg("-B")
        .arg(build_dir)
        .current_dir(&dir);
    if let Some(generator) = configuration.cmake_generator(settings) {
        command = command.arg("-G").arg(generator);
    }
    if !configuration.build_type.is_empty() {
        command = command.arg(format!("-DCMAKE_BUILD_TYPE={}", configuration.build_type));
    }
    for arg in split_args(&configuration.configure_args)? {
        command = command.arg(arg);
    }
    with_environment(command, &configuration.environment)
}

fn with_environment(mut command: Command, environment: &str) -> Result<Command, String> {
    for (name, value) in parse_environment(environment)? {
        command = command.env(name, value);
    }
    Ok(command)
}

/// Split a command line into arguments the way a shell would, less the
/// expansions: whitespace separates arguments, quotes (single or double)
/// group text that contains it, and a backslash outside single quotes
/// keeps the character after it.
pub fn split_args(text: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_arg = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            '\'' => {
                in_arg = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("unclosed ' quote".to_owned()),
                    }
                }
            }
            '"' => {
                in_arg = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => current.push(c),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => return Err("unclosed \" quote".to_owned()),
                        },
                        Some(c) => current.push(c),
                        None => return Err("unclosed \" quote".to_owned()),
                    }
                }
            }
            '\\' => {
                in_arg = true;
                match chars.next() {
                    Some(c) => current.push(c),
                    None => return Err("a backslash needs a character after it".to_owned()),
                }
            }
            c => {
                in_arg = true;
                current.push(c);
            }
        }
    }
    if in_arg {
        args.push(current);
    }
    Ok(args)
}

/// Parse environment variables typed as `NAME=value` pairs separated by
/// spaces, quoted like arguments where a value has spaces in it.
pub fn parse_environment(text: &str) -> Result<Vec<(String, String)>, String> {
    split_args(text)?
        .into_iter()
        .map(|entry| match entry.split_once('=') {
            Some((name, value)) if !name.is_empty() => Ok((name.to_owned(), value.to_owned())),
            _ => Err(format!("{entry} should be NAME=value")),
        })
        .collect()
}

// ----- Discovery ------------------------------------------------------------

/// What discovery found under a root: the targets, and which of them a
/// fresh root should start on, if discovery has an opinion.
#[derive(Clone, Debug)]
struct Discovered {
    targets: Vec<Target>,
    preferred: Option<String>,
}

/// The work of finding a root's targets by looking at the project,
/// which for a CMake root means reading every `CMakeLists.txt` under it
/// and for a big tree takes a while. Handed out by
/// [`BuildRoot::discovery`] and [`BuildConfig::request_discovery`], run
/// on whatever thread suits with [`run`](Self::run), and the result
/// taken in with [`BuildConfig::apply_discovery`].
#[derive(Clone, Debug)]
pub struct Discovery {
    /// The root's path relative to the project: its identity.
    path: PathBuf,
    system: BuildSystem,
    /// The root file, absolute.
    file: PathBuf,
}

impl Discovery {
    /// The root this is for, relative to the project.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Look at the project. `files` is the project's file list where one
    /// is at hand (the file index's, with ignored files left out), which
    /// saves walking the tree again; without it the tree under the root
    /// is walked here.
    pub fn run(self, files: Option<&[PathBuf]>) -> DiscoveryResult {
        let found = match self.system {
            BuildSystem::Cargo => cargo_targets(&self.file),
            BuildSystem::CMake => cmake_targets(&self.file, files),
        };
        DiscoveryResult {
            path: self.path,
            found,
        }
    }
}

/// What a [`Discovery`] found, for [`BuildConfig::apply_discovery`].
#[derive(Clone, Debug)]
pub struct DiscoveryResult {
    path: PathBuf,
    found: Discovered,
}

impl DiscoveryResult {
    /// The root this is for, relative to the project.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The targets for a Cargo root: one per package the manifest names,
/// the root package first and then the workspace members. A manifest
/// that names none (or can't be read) gets one target for the whole
/// workspace. The preferred target is the first package with a binary
/// among the workspace's default members, else the first with a binary
/// at all, else the first default member.
fn cargo_targets(manifest: &Path) -> Discovered {
    let packages = cargo_packages(manifest);
    let preferred = packages
        .iter()
        .find(|p| p.has_binary && p.default_member)
        .or_else(|| packages.iter().find(|p| p.has_binary))
        .or_else(|| packages.iter().find(|p| p.default_member))
        .map(|p| p.name.clone());
    let targets: Vec<Target> = packages
        .into_iter()
        .map(|package| Target {
            name: package.name.clone(),
            package: package.name.clone(),
            auto: Some(package.name),
            ..Target::default()
        })
        .collect();
    if targets.is_empty() {
        let name = manifest
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "All".to_owned());
        return Discovered {
            targets: vec![Target {
                name,
                auto: Some(ALL_TARGET_ID.to_owned()),
                ..Target::default()
            }],
            preferred: None,
        };
    }
    Discovered { targets, preferred }
}

/// A package a Cargo manifest describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CargoPackage {
    pub name: String,
    /// Whether the package builds a binary: it has `[[bin]]` targets, or
    /// a `src/main.rs` or `src/bin/` that Cargo finds by itself.
    pub has_binary: bool,
    /// Whether a plain `cargo build` in the workspace builds it: it is
    /// among the workspace's `default-members`, or, with none listed,
    /// it is the root package.
    pub default_member: bool,
}

/// The packages a manifest describes: its own package, if it has one,
/// then each workspace member's, member globs expanded and exclusions
/// honored.
pub fn cargo_packages(manifest: &Path) -> Vec<CargoPackage> {
    let Ok(text) = fs::read_to_string(manifest) else {
        return Vec::new();
    };
    let Ok(table) = text.parse::<Table>() else {
        return Vec::new();
    };
    let dir = manifest.parent().unwrap_or(Path::new(""));
    let workspace = table.get("workspace").and_then(Value::as_table);
    let patterns = |key: &str| -> Vec<String> {
        workspace
            .and_then(|w| w.get(key))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let default_members: Option<Vec<PathBuf>> = workspace
        .is_some_and(|w| w.contains_key("default-members"))
        .then(|| {
            patterns("default-members")
                .iter()
                .flat_map(|pattern| expand_glob(dir, pattern))
                .collect()
        });
    let is_default = |member_dir: &Path, is_root: bool| match &default_members {
        Some(defaults) => defaults.iter().any(|d| d == member_dir),
        None => is_root,
    };
    let mut packages = Vec::new();
    let mut seen = HashSet::new();
    if let Some(name) = package_name(&table) {
        seen.insert(name.clone());
        packages.push(CargoPackage {
            name,
            has_binary: has_binary(dir, &table),
            default_member: is_default(dir, true),
        });
    }
    if workspace.is_none() {
        return packages;
    };
    let excluded: Vec<PathBuf> = patterns("exclude")
        .iter()
        .flat_map(|pattern| expand_glob(dir, pattern))
        .collect();
    for pattern in patterns("members") {
        for member in expand_glob(dir, &pattern) {
            if excluded.contains(&member) {
                continue;
            }
            let Ok(text) = fs::read_to_string(member.join("Cargo.toml")) else {
                continue;
            };
            let Ok(table) = text.parse::<Table>() else {
                continue;
            };
            if let Some(name) = package_name(&table)
                && seen.insert(name.clone())
            {
                packages.push(CargoPackage {
                    name,
                    has_binary: has_binary(&member, &table),
                    default_member: is_default(&member, false),
                });
            }
        }
    }
    packages
}

fn package_name(manifest: &Table) -> Option<String> {
    manifest
        .get("package")?
        .as_table()?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// Whether a package in `dir` builds a binary: it declares `[[bin]]`
/// targets, or (unless `autobins` is off) has a `src/main.rs` or a
/// `src/bin/` with a `.rs` file or a directory with a `main.rs` in it.
fn has_binary(dir: &Path, manifest: &Table) -> bool {
    if manifest
        .get("bin")
        .and_then(Value::as_array)
        .is_some_and(|bins| !bins.is_empty())
    {
        return true;
    }
    let autobins = manifest
        .get("package")
        .and_then(Value::as_table)
        .and_then(|p| p.get("autobins"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !autobins {
        return false;
    }
    if dir.join("src").join("main.rs").is_file() {
        return true;
    }
    let Ok(entries) = fs::read_dir(dir.join("src").join("bin")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        (path.is_file() && path.extension().is_some_and(|e| e == "rs"))
            || (path.is_dir() && path.join("main.rs").is_file())
    })
}

/// The directories a workspace member pattern names under `dir`: the
/// path itself when it has no wildcard, else every directory matching,
/// `*` standing for any run of characters within one path component.
/// Matches come sorted, so the order is the same from run to run.
fn expand_glob(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut paths = vec![dir.to_path_buf()];
    for component in Path::new(pattern).components() {
        let Component::Normal(part) = component else {
            // `..`, `.`, or a root: take it as written.
            paths = paths.into_iter().map(|p| p.join(component)).collect();
            continue;
        };
        let part = part.to_string_lossy();
        if !part.contains(['*', '?']) {
            paths = paths.into_iter().map(|p| p.join(&*part)).collect();
            continue;
        }
        let mut next = Vec::new();
        for path in paths {
            let Ok(entries) = fs::read_dir(&path) else {
                continue;
            };
            let mut matches: Vec<PathBuf> = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .filter(|entry| glob_matches(&part, &entry.file_name().to_string_lossy()))
                .map(|entry| entry.path())
                .collect();
            matches.sort();
            next.extend(matches);
        }
        paths = next;
    }
    paths.into_iter().filter(|p| p.is_dir()).collect()
}

/// Whether a name matches a pattern of literal characters, `*` (any run
/// of characters) and `?` (any one character).
fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    // dp[i][j]: the first i pattern chars match the first j name chars.
    let mut dp = vec![vec![false; name.len() + 1]; pattern.len() + 1];
    dp[0][0] = true;
    for i in 1..=pattern.len() {
        if pattern[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
        for j in 1..=name.len() {
            dp[i][j] = match pattern[i - 1] {
                '*' => dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == name[j - 1],
            };
        }
    }
    dp[pattern.len()][name.len()]
}

/// The targets for a CMake root: one to build everything, then one per
/// executable declared in the tree's `CMakeLists.txt` files, each set
/// to run that executable from where its directory puts it in the build
/// directory.
fn cmake_targets(lists: &Path, project_files: Option<&[PathBuf]>) -> Discovered {
    let mut targets = vec![cmake_all_target()];
    let dir = lists.parent().unwrap_or(Path::new(""));
    let files = match project_files {
        Some(project_files) => cmake_lists_among(dir, project_files),
        None => {
            let mut files = Vec::new();
            collect_cmake_lists(dir, dir, &mut files);
            files
        }
    };
    let mut seen = HashSet::new();
    for (relative, file) in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for name in cmake_executables(&text) {
            if !seen.insert(name.clone()) {
                continue;
            }
            let executable = if relative.as_os_str().is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", slash_path(&relative))
            };
            targets.push(Target {
                name: name.clone(),
                cmake_target: name.clone(),
                executable,
                auto: Some(name),
                ..Target::default()
            });
        }
    }
    Discovered {
        targets,
        preferred: None,
    }
}

/// The target for everything a CMake root builds.
fn cmake_all_target() -> Target {
    Target {
        name: "All".to_owned(),
        auto: Some(ALL_TARGET_ID.to_owned()),
        ..Target::default()
    }
}

/// Whether a directory is left out of the search for `CMakeLists.txt`
/// files: build directories and hidden ones.
fn skipped_dir(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    name.starts_with(b".")
        || name.starts_with(CMAKE_BUILD_DIR_PREFIX.as_bytes())
        || name == b"build"
        || name == b"target"
}

/// Every `CMakeLists.txt` under `dir`, as (directory relative to `dir`,
/// file path), the top one first and the rest in path order. Build
/// directories and hidden directories are left out, and symbolic links
/// aren't followed. Each entry costs one directory listing and nothing
/// more: on a tree of tens of thousands of directories, a stat for each
/// entry is most of the time.
fn collect_cmake_lists(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = Vec::new();
    let mut has_lists = false;
    for entry in entries.flatten() {
        // The entry's own type: a symbolic link is neither, so a link
        // cycle can't trap the walk.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        if file_type.is_dir() {
            if !skipped_dir(&name) {
                children.push(entry.path());
            }
        } else if file_type.is_file() && name == BuildSystem::CMake.file_name() {
            has_lists = true;
        }
    }
    if has_lists {
        let relative = dir.strip_prefix(root).unwrap_or(dir).to_path_buf();
        out.push((relative, dir.join(BuildSystem::CMake.file_name())));
    }
    children.sort();
    for child in children {
        collect_cmake_lists(root, &child, out);
    }
}

/// The `CMakeLists.txt` files under `dir` among a project's files, as
/// [`collect_cmake_lists`] would list them.
fn cmake_lists_among(dir: &Path, files: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let mut out: Vec<(PathBuf, PathBuf)> = files
        .iter()
        .filter(|file| {
            file.file_name()
                .is_some_and(|name| name == BuildSystem::CMake.file_name())
        })
        .filter_map(|file| {
            let relative = file.parent()?.strip_prefix(dir).ok()?;
            let skipped = relative
                .components()
                .any(|component| skipped_dir(component.as_os_str()));
            (!skipped).then(|| (relative.to_path_buf(), file.clone()))
        })
        .collect();
    out.sort();
    out
}

/// The names of the executables a `CMakeLists.txt` declares with
/// `add_executable`, leaving out aliases, imported ones, and names made
/// from variables.
fn cmake_executables(text: &str) -> Vec<String> {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?im)^\s*add_executable\s*\(\s*([A-Za-z0-9_.+-]+)\s*([A-Za-z_]*)").unwrap()
    });
    PATTERN
        .captures_iter(text)
        .filter(|caps| {
            let kind = caps.get(2).map_or("", |m| m.as_str());
            !kind.eq_ignore_ascii_case("ALIAS") && !kind.eq_ignore_ascii_case("IMPORTED")
        })
        .map(|caps| caps[1].to_owned())
        .collect()
}

/// The root files in a list of project paths: every `Cargo.toml` and
/// `CMakeLists.txt`, in path order, as a frontend lists them when a root
/// is added.
pub fn root_candidates<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = paths
        .into_iter()
        .filter(|path| BuildSystem::of_file(path).is_some())
        .cloned()
        .collect();
    candidates.sort();
    candidates
}

/// A command's arguments as `OsString`s, for tests and display.
pub fn command_args(command: &Command) -> Vec<OsString> {
    command.args().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingKey;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn splits_arguments_like_a_shell() {
        assert_eq!(split_args("").unwrap(), Vec::<String>::new());
        assert_eq!(split_args("  a  b ").unwrap(), vec!["a", "b"]);
        assert_eq!(
            split_args(r#"-DFOO="a b" 'c d' e\ f "x\"y""#).unwrap(),
            vec!["-DFOO=a b", "c d", "e f", "x\"y"]
        );
        assert_eq!(split_args("\"\"").unwrap(), vec![""]);
        assert_eq!(split_args("a'b'c").unwrap(), vec!["abc"]);
        assert_eq!(split_args("'a").unwrap_err(), "unclosed ' quote");
        assert_eq!(split_args("\"a").unwrap_err(), "unclosed \" quote");
        assert!(split_args("a\\").is_err());
    }

    #[test]
    fn parses_environment_pairs() {
        assert_eq!(parse_environment("").unwrap(), vec![]);
        assert_eq!(
            parse_environment("A=1 B=\"two words\" C=").unwrap(),
            vec![
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "two words".to_owned()),
                ("C".to_owned(), String::new()),
            ]
        );
        assert_eq!(
            parse_environment("A=1 B").unwrap_err(),
            "B should be NAME=value"
        );
        assert_eq!(
            parse_environment("=1").unwrap_err(),
            "=1 should be NAME=value"
        );
    }

    #[test]
    fn globs_match_within_a_component() {
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("crates-*", "crates-a"));
        assert!(!glob_matches("crates-*", "crate-a"));
        assert!(glob_matches("a?c", "abc"));
        assert!(!glob_matches("a?c", "ac"));
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(glob_matches("", ""));
        assert!(!glob_matches("", "a"));
    }

    #[test]
    fn discovers_a_cargo_workspace_and_its_members() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"core\", \"crates/*\", \"missing\"]\nexclude = [\"crates/skip\"]\n\n[package]\nname = \"top\"\n",
        );
        write(
            &root.join("core/Cargo.toml"),
            "[package]\nname = \"core-lib\"\n",
        );
        write(
            &root.join("crates/b/Cargo.toml"),
            "[package]\nname = \"b\"\n",
        );
        write(
            &root.join("crates/a/Cargo.toml"),
            "[package]\nname = \"a\"\n",
        );
        write(
            &root.join("crates/skip/Cargo.toml"),
            "[package]\nname = \"skip\"\n",
        );
        write(&root.join("crates/notes.txt"), "not a package");
        let packages = cargo_packages(&root.join("Cargo.toml"));
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["top", "core-lib", "a", "b"]);

        let config = BuildConfig::discover(root);
        assert_eq!(config.roots().len(), 1);
        let cargo = &config.roots()[0];
        assert_eq!(cargo.system(), BuildSystem::Cargo);
        assert_eq!(cargo.path(), Path::new("Cargo.toml"));
        assert_eq!(cargo.directory(), Path::new(""));
        let names: Vec<&str> = cargo
            .configurations()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["dev", "release"]);
        let targets: Vec<(&str, &str)> = cargo
            .targets()
            .iter()
            .map(|t| (t.name.as_str(), t.package.as_str()))
            .collect();
        assert_eq!(
            targets,
            vec![
                ("top", "top"),
                ("core-lib", "core-lib"),
                ("a", "a"),
                ("b", "b")
            ]
        );
        assert_eq!(
            config.current(),
            Some(Selection {
                root: 0,
                configuration: 0,
                target: 0
            })
        );

        // A manifest without packages gets one target for everything,
        // named after its directory.
        write(&root.join("empty/Cargo.toml"), "[workspace]\n");
        let empty = BuildRoot::discover(root, "empty/Cargo.toml").unwrap();
        assert_eq!(empty.targets().len(), 1);
        assert_eq!(empty.targets()[0].name, "empty");
        assert_eq!(empty.targets()[0].package, "");
        assert_eq!(empty.directory(), Path::new("empty"));
        assert!(BuildRoot::discover(root, "empty/notes.txt").is_none());
    }

    #[test]
    fn a_fresh_cargo_root_starts_on_a_binary_package_preferring_default_members() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A library first, then two binaries; `default-members` names
        // the second binary.
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"lib\", \"app\", \"cli\", \"tool\"]\ndefault-members = [\"lib\", \"cli\"]\n",
        );
        write(&root.join("lib/Cargo.toml"), "[package]\nname = \"lib\"\n");
        write(&root.join("lib/src/lib.rs"), "");
        write(&root.join("app/Cargo.toml"), "[package]\nname = \"app\"\n");
        write(&root.join("app/src/main.rs"), "fn main() {}");
        write(
            &root.join("cli/Cargo.toml"),
            "[package]\nname = \"cli\"\n[[bin]]\nname = \"cli\"\npath = \"cli.rs\"\n",
        );
        write(
            &root.join("tool/Cargo.toml"),
            "[package]\nname = \"tool\"\nautobins = false\n",
        );
        write(&root.join("tool/src/main.rs"), "fn main() {}");
        let packages = cargo_packages(&root.join("Cargo.toml"));
        let summary: Vec<(&str, bool, bool)> = packages
            .iter()
            .map(|p| (p.name.as_str(), p.has_binary, p.default_member))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("lib", false, true),
                ("app", true, false),
                ("cli", true, true),
                ("tool", false, false)
            ]
        );
        let config = BuildConfig::discover(root);
        assert_eq!(config.current_target().unwrap().1.name, "cli");
        assert_eq!(config.roots()[0].default_target(), 2);

        // Without default members, the first binary; `src/bin/` counts.
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"lib\", \"app\", \"cli\"]\n",
        );
        assert_eq!(
            BuildConfig::discover(root).current_target().unwrap().1.name,
            "app"
        );
        std::fs::remove_file(root.join("app/src/main.rs")).unwrap();
        write(&root.join("app/src/bin/serve.rs"), "fn main() {}");
        assert_eq!(
            BuildConfig::discover(root).current_target().unwrap().1.name,
            "app"
        );
        std::fs::remove_dir_all(root.join("app/src/bin")).unwrap();
        assert_eq!(
            BuildConfig::discover(root).current_target().unwrap().1.name,
            "cli"
        );

        // A root package with no default members listed is the default
        // member, but a binary elsewhere still wins over a root library.
        write(
            &root.join("Cargo.toml"),
            "[package]\nname = \"top\"\n[workspace]\nmembers = [\"lib\", \"cli\"]\n",
        );
        write(&root.join("src/lib.rs"), "");
        let packages = cargo_packages(&root.join("Cargo.toml"));
        assert!(packages[0].default_member && !packages[0].has_binary);
        assert!(!packages[1].default_member);
        assert_eq!(
            BuildConfig::discover(root).current_target().unwrap().1.name,
            "cli"
        );
        // With nothing runnable, the first default member.
        write(
            &root.join("Cargo.toml"),
            "[package]\nname = \"top\"\n[workspace]\nmembers = [\"lib\"]\n",
        );
        assert_eq!(
            BuildConfig::discover(root).current_target().unwrap().1.name,
            "top"
        );
        // A disabled preferred target isn't started on.
        let mut config = BuildConfig::discover(root);
        config.set_target_disabled(0, 0, true);
        assert_eq!(config.roots()[0].default_target(), 1);
    }

    #[test]
    fn discovers_cmake_executables_through_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("CMakeLists.txt"),
            "project(demo)\nadd_executable(demo main.c)\nadd_executable(demo::alias ALIAS demo)\nadd_executable(${PROJECT_NAME}_x x.c)\nadd_subdirectory(tools)\n",
        );
        write(
            &root.join("tools/CMakeLists.txt"),
            "ADD_EXECUTABLE( helper helper.c )\nadd_executable(ext IMPORTED)\n",
        );
        write(
            &root.join("cmake-build-debug/CMakeLists.txt"),
            "add_executable(generated g.c)\n",
        );
        write(
            &root.join(".hidden/CMakeLists.txt"),
            "add_executable(hidden h.c)\n",
        );
        let config = BuildConfig::discover(root);
        let cmake = &config.roots()[0];
        assert_eq!(cmake.system(), BuildSystem::CMake);
        let names: Vec<&str> = cmake
            .configurations()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["Debug", "Release"]);
        assert_eq!(cmake.configurations()[0].build_type, "Debug");
        assert_eq!(
            cmake.configurations()[0].cmake_build_dir_name(),
            "cmake-build-debug"
        );
        let targets: Vec<(&str, &str, &str)> = cmake
            .targets()
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.cmake_target.as_str(),
                    t.executable.as_str(),
                )
            })
            .collect();
        assert_eq!(
            targets,
            vec![
                ("All", "", ""),
                ("demo", "demo", "demo"),
                ("helper", "helper", "tools/helper"),
            ]
        );

        // A root pending discovery has `All` before anything is looked
        // at; given the project's files, as the index lists them, the
        // discovery finds the same targets without walking the tree.
        let pending = BuildRoot::pending("CMakeLists.txt").unwrap();
        assert!(pending.is_pending());
        assert_eq!(pending.targets().len(), 1);
        assert_eq!(pending.targets()[0].name, "All");
        let files: Vec<PathBuf> = [
            "tools/helper.c",
            "tools/CMakeLists.txt",
            "cmake-build-debug/CMakeLists.txt",
            ".hidden/CMakeLists.txt",
            "CMakeLists.txt",
        ]
        .iter()
        .map(|f| root.join(f))
        .collect();
        let result = pending.discovery(root).run(Some(&files));
        assert_eq!(result.path(), Path::new("CMakeLists.txt"));
        let mut from_files = BuildConfig::default();
        from_files.add_root(pending).unwrap();
        assert!(from_files.is_discovering());
        assert_eq!(from_files.apply_discovery(result), Some(0));
        assert!(!from_files.is_discovering());
        assert_eq!(from_files.roots()[0].targets(), cmake.targets());
    }

    #[test]
    fn both_roots_at_the_top_are_found_and_others_can_be_added() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("Cargo.toml"), "[package]\nname = \"app\"\n");
        write(&root.join("CMakeLists.txt"), "add_executable(app app.c)\n");
        write(
            &root.join("native/lib/CMakeLists.txt"),
            "add_executable(lib lib.c)\n",
        );
        let mut config = BuildConfig::discover(root);
        assert_eq!(config.roots().len(), 2);
        assert_eq!(config.roots()[0].system(), BuildSystem::Cargo);
        assert_eq!(config.roots()[1].system(), BuildSystem::CMake);

        let nested =
            BuildRoot::discover(root, Path::new("native").join("lib").join("CMakeLists.txt"))
                .unwrap();
        assert_eq!(nested.label(), "native/lib/CMakeLists.txt");
        assert_eq!(nested.targets()[1].executable, "lib");
        assert_eq!(config.add_root(nested.clone()), Ok(2));
        assert_eq!(
            config.add_root(nested).unwrap_err(),
            "native/lib/CMakeLists.txt is already a build root"
        );
        assert!(
            BuildConfig::discover(&root.join("native"))
                .roots()
                .is_empty()
        );

        let files = [
            PathBuf::from("z/CMakeLists.txt"),
            PathBuf::from("Cargo.toml"),
            PathBuf::from("src/main.rs"),
            PathBuf::from("a/Cargo.toml"),
        ];
        assert_eq!(
            root_candidates(files.iter()),
            vec![
                PathBuf::from("Cargo.toml"),
                PathBuf::from("a/Cargo.toml"),
                PathBuf::from("z/CMakeLists.txt")
            ]
        );
    }

    fn two_roots() -> BuildConfig {
        let mut config = BuildConfig::default();
        let mut cargo = BuildRoot::new(BuildSystem::Cargo, "Cargo.toml");
        cargo.configurations = BuildSystem::Cargo.default_configurations();
        cargo.targets = vec![
            Target {
                name: "app".to_owned(),
                package: "app".to_owned(),
                ..Target::default()
            },
            Target {
                name: "tool".to_owned(),
                package: "tool".to_owned(),
                ..Target::default()
            },
        ];
        let mut cmake = BuildRoot::new(BuildSystem::CMake, "native/CMakeLists.txt");
        cmake.configurations = BuildSystem::CMake.default_configurations();
        cmake.targets = vec![
            Target {
                name: "All".to_owned(),
                ..Target::default()
            },
            Target {
                name: "tool".to_owned(),
                cmake_target: "tool".to_owned(),
                executable: "bin/tool".to_owned(),
                ..Target::default()
            },
        ];
        config.add_root(cargo).unwrap();
        config.add_root(cmake).unwrap();
        config
    }

    #[test]
    fn selecting_across_roots_brings_the_other_half_along() {
        let mut config = two_roots();
        assert!(config.select_target(0, 1));
        assert_eq!(config.current_target().unwrap().1.name, "tool");
        assert!(!config.select_target(0, 1), "unchanged");
        // A configuration from the CMake root: the target of the same
        // name in that root comes along.
        assert!(config.select_configuration(1, 1));
        let (root, configuration) = config.current_configuration().unwrap();
        assert_eq!(root.system(), BuildSystem::CMake);
        assert_eq!(configuration.name, "Release");
        assert_eq!(config.current_target().unwrap().1.name, "tool");
        // Back to Cargo by target: no `Release` there (case aside), so
        // its first configuration.
        assert!(config.select_target(0, 0));
        assert_eq!(config.current_configuration().unwrap().1.name, "dev");
        assert_eq!(config.current_target().unwrap().1.name, "app");
        // A configuration with no matching target lands on the first.
        assert!(config.select_configuration(1, 0));
        assert_eq!(config.current_target().unwrap().1.name, "All");
        assert!(!config.select_configuration(5, 0), "no such root");
    }

    #[test]
    fn a_target_can_be_run_without_becoming_current() {
        let mut config = two_roots();
        let project = Path::new("/proj");
        let settings = Settings::default();
        config.select_configuration(0, 1);
        let before = config.current().unwrap();
        assert_eq!(config.current_target().unwrap().1.name, "app");

        // Another target in the same root runs with the current
        // configuration.
        let selection = config.selection_with_target(0, 1).unwrap();
        assert_eq!(
            config.configuration_for(selection).unwrap().1.name,
            "release"
        );
        let job = config
            .run_job_for(selection, project, false, &settings)
            .unwrap();
        assert_eq!(job.title, "Run tool (release)");
        assert!(args(&job.steps[0].command).contains(&"--release".to_owned()));
        assert!(config.cmake_build_dir_for(project, selection).is_none());

        // One in another root runs with that root's configuration of the
        // same name, or its first; the build directory follows.
        let selection = config.selection_with_target(1, 1).unwrap();
        assert_eq!(config.configuration_for(selection).unwrap().1.name, "Debug");
        let job = config
            .run_job_for(selection, project, true, &settings)
            .unwrap();
        assert_eq!(job.title, "Run tool (Debug)");
        assert_eq!(job.steps.len(), 3, "configure, build, run");
        assert_eq!(
            config.cmake_build_dir_for(project, selection),
            Some(PathBuf::from("/proj/native/cmake-build-debug"))
        );
        let build = config
            .build_job_for(selection, project, false, &settings)
            .unwrap();
        assert_eq!(build.title, "Build tool (Debug)");

        // Nothing moved.
        assert_eq!(config.current(), Some(before));
        assert_eq!(config.current_target().unwrap().1.name, "app");
        assert!(config.selection_with_target(5, 0).is_none(), "no such root");
        // And selecting it for real lands on the same selection.
        assert!(config.select_target(1, 1));
        assert_eq!(config.current(), Some(selection));
    }

    #[test]
    fn adding_duplicating_and_removing_keeps_the_selection_sensible() {
        let mut config = two_roots();
        config.select_target(0, 1);
        assert_eq!(config.add_configuration(0), Some(2));
        assert_eq!(
            config.roots()[0].configurations()[2].name,
            "New configuration"
        );
        assert_eq!(config.add_configuration(0), Some(3));
        assert_eq!(
            config.roots()[0].configurations()[3].name,
            "New configuration 2"
        );
        assert_eq!(config.duplicate_configuration(0, 1), Some(2));
        let copy = &config.roots()[0].configurations()[2];
        assert_eq!(copy.name, "release copy");
        assert_eq!(copy.profile, "release");
        assert_eq!(config.duplicate_configuration(0, 1), Some(2));
        assert_eq!(config.roots()[0].configurations()[2].name, "release copy 2");
        assert_eq!(config.duplicate_configuration(0, 9), None);

        // Removing a target before the current one shifts it down;
        // removing the current one moves to the nearest.
        assert_eq!(config.current().unwrap().target, 1);
        config.remove_target(0, 0).unwrap();
        assert_eq!(config.current().unwrap().target, 0);
        assert_eq!(config.current_target().unwrap().1.name, "tool");
        config.remove_target(0, 0).unwrap();
        assert!(config.current_target().is_none());
        assert_eq!(config.add_target(0), Some(0));
        assert_eq!(config.current_target().unwrap().1.name, "New target");
        assert_eq!(config.duplicate_target(0, 0), Some(1));
        assert_eq!(config.roots()[0].targets()[1].name, "New target copy");

        // Removing the current root falls back to the first root.
        config.select_configuration(1, 1);
        config.remove_root(1);
        assert_eq!(config.roots().len(), 1);
        assert_eq!(config.current().unwrap().root, 0);
        config.remove_root(0);
        assert_eq!(config.current(), None);
        assert!(config.current_configuration().is_none());
        config.remove_root(0);
    }

    #[test]
    fn options_are_checked_as_they_are_set() {
        let mut config = two_roots();
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::Name, " fast "),
            Ok(true)
        );
        assert_eq!(
            config.configuration_text(0, 0, ConfigurationKey::Name),
            "fast"
        );
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::Name, "fast"),
            Ok(false)
        );
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::Name, ""),
            Err("enter a name".to_owned())
        );
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::Name, "RELEASE"),
            Err("there is already a configuration named RELEASE".to_owned())
        );
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::Name, "a/b"),
            Err("a name can't contain / or \\".to_owned())
        );
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::BuildArgs, "--features 'x"),
            Err("unclosed ' quote".to_owned())
        );
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::Environment, "RUST_LOG"),
            Err("RUST_LOG should be NAME=value".to_owned())
        );
        assert_eq!(
            config.set_configuration_text(0, 0, ConfigurationKey::Environment, "RUST_LOG=debug"),
            Ok(true)
        );
        assert_eq!(
            config.set_target_text(0, 1, TargetKey::Name, "app"),
            Err("there is already a target named app".to_owned())
        );
        assert_eq!(
            config.set_target_text(0, 1, TargetKey::Name, "tool"),
            Ok(false)
        );
        assert_eq!(
            config.set_target_text(0, 1, TargetKey::Arguments, "--port 80"),
            Ok(true)
        );
        assert_eq!(config.target_text(0, 1, TargetKey::Arguments), "--port 80");
        assert!(config.set_target_text(3, 0, TargetKey::Name, "x").is_err());
        assert!(config.set_target_text(0, 9, TargetKey::Name, "x").is_err());
    }

    #[test]
    fn cargo_jobs_build_and_run_the_package_with_the_profile() {
        let mut config = two_roots();
        let project = Path::new("/proj");
        let job = config
            .build_job(project, true, &Settings::default())
            .unwrap();
        assert_eq!(job.title, "Build app (dev)");
        assert_eq!(job.steps.len(), 1);
        let step = &job.steps[0];
        assert_eq!(step.command.program(), "cargo");
        assert_eq!(args(&step.command), vec!["build", "-p", "app"]);
        assert_eq!(step.command.current_dir_path(), Some(project));

        config.select_configuration(0, 1);
        config
            .set_configuration_text(0, 1, ConfigurationKey::BuildArgs, "--features \"a b\"")
            .unwrap();
        config
            .set_configuration_text(0, 1, ConfigurationKey::Environment, "RUSTFLAGS=-Dwarnings")
            .unwrap();
        config
            .set_target_text(0, 0, TargetKey::Binary, "app-cli")
            .unwrap();
        config
            .set_target_text(0, 0, TargetKey::Arguments, "serve --port 80")
            .unwrap();
        config
            .set_target_text(0, 0, TargetKey::WorkingDirectory, "www")
            .unwrap();
        config
            .set_target_text(0, 0, TargetKey::Environment, "PORT=80")
            .unwrap();
        let job = config
            .build_job(project, false, &Settings::default())
            .unwrap();
        assert_eq!(
            args(&job.steps[0].command),
            vec![
                "build",
                "--release",
                "-p",
                "app",
                "--bin",
                "app-cli",
                "--features",
                "a b"
            ]
        );
        assert_eq!(
            job.steps[0].command.envs(),
            &[(OsString::from("RUSTFLAGS"), OsString::from("-Dwarnings"))]
        );
        let job = config
            .run_job(project, false, &Settings::default())
            .unwrap();
        assert_eq!(job.title, "Run app (release)");
        assert_eq!(job.steps.len(), 1);
        let run = &job.steps[0].command;
        assert_eq!(
            args(run),
            vec![
                "run",
                "--release",
                "--manifest-path",
                "/proj/Cargo.toml",
                "-p",
                "app",
                "--bin",
                "app-cli",
                "--features",
                "a b",
                "--",
                "serve",
                "--port",
                "80"
            ]
        );
        assert_eq!(run.current_dir_path(), Some(Path::new("/proj/www")));
        assert_eq!(run.envs().len(), 2);

        // A custom profile goes through --profile.
        config
            .set_configuration_text(0, 1, ConfigurationKey::Profile, "bench")
            .unwrap();
        let job = config
            .build_job(project, false, &Settings::default())
            .unwrap();
        assert_eq!(args(&job.steps[0].command)[1..3], ["--profile", "bench"]);
    }

    #[test]
    fn cmake_jobs_configure_build_and_run_in_the_named_directory() {
        let mut config = two_roots();
        config.select_target(1, 1);
        let project = Path::new("/proj");
        assert_eq!(
            config.cmake_build_dir(project),
            Some(PathBuf::from("/proj/native/cmake-build-debug"))
        );
        config
            .set_configuration_text(1, 0, ConfigurationKey::ConfigureArgs, "-DFOO=ON -G Ninja")
            .unwrap();
        config
            .set_configuration_text(1, 0, ConfigurationKey::BuildArgs, "-j 4")
            .unwrap();
        config
            .set_configuration_text(1, 0, ConfigurationKey::Environment, "CC=clang")
            .unwrap();
        config
            .set_target_text(1, 1, TargetKey::Arguments, "--fast")
            .unwrap();
        config
            .set_target_text(1, 1, TargetKey::Environment, "DEBUG=1")
            .unwrap();

        let job = config
            .build_job(project, true, &Settings::default())
            .unwrap();
        assert_eq!(job.title, "Build tool (Debug)");
        let descriptions: Vec<&str> = job.steps.iter().map(|s| s.description.as_str()).collect();
        assert_eq!(descriptions, vec!["Configuring Debug", "Building tool"]);
        let configure = &job.steps[0].command;
        assert_eq!(configure.program(), "cmake");
        assert_eq!(
            args(configure),
            vec![
                "-S",
                "/proj/native",
                "-B",
                "/proj/native/cmake-build-debug",
                "-DCMAKE_BUILD_TYPE=Debug",
                "-DFOO=ON",
                "-G",
                "Ninja"
            ],
            "the configuration's own -G stands in for the setting's"
        );
        assert_eq!(
            configure.current_dir_path(),
            Some(Path::new("/proj/native"))
        );
        assert_eq!(
            configure.envs(),
            &[(OsString::from("CC"), OsString::from("clang"))]
        );
        let build = &job.steps[1].command;
        assert_eq!(
            args(build),
            vec![
                "--build",
                "/proj/native/cmake-build-debug",
                "--target",
                "tool",
                "-j",
                "4"
            ]
        );

        // Without configuring, and with everything to build.
        config.select_target(1, 0);
        let job = config
            .build_job(project, false, &Settings::default())
            .unwrap();
        assert_eq!(job.steps.len(), 1);
        assert_eq!(
            args(&job.steps[0].command),
            vec!["--build", "/proj/native/cmake-build-debug", "-j", "4"]
        );
        assert_eq!(
            config
                .run_job(project, false, &Settings::default())
                .unwrap_err(),
            "All has no executable to run: set one on the build configuration page"
        );

        config.select_target(1, 1);
        let job = config
            .run_job(project, false, &Settings::default())
            .unwrap();
        assert_eq!(job.title, "Run tool (Debug)");
        assert_eq!(job.steps.len(), 2);
        let run = &job.steps[1].command;
        assert_eq!(
            run.program(),
            Path::new("/proj/native/cmake-build-debug/bin/tool").as_os_str()
        );
        assert_eq!(args(run), vec!["--fast"]);
        assert_eq!(run.current_dir_path(), Some(Path::new("/proj/native")));
        assert_eq!(run.envs().len(), 2, "the configuration's and the target's");
        config
            .set_target_text(1, 1, TargetKey::WorkingDirectory, "data")
            .unwrap();
        let job = config
            .run_job(project, false, &Settings::default())
            .unwrap();
        assert_eq!(
            job.steps[1].command.current_dir_path(),
            Some(Path::new("/proj/native/data"))
        );

        // A blank build type passes none.
        config
            .set_configuration_text(1, 0, ConfigurationKey::BuildType, "")
            .unwrap();
        let job = config
            .build_job(project, true, &Settings::default())
            .unwrap();
        assert!(
            !args(&job.steps[0].command)
                .iter()
                .any(|a| a.contains("CMAKE_BUILD_TYPE"))
        );
        assert_ne!(
            config.roots()[1].configurations()[0].cmake_configure_signature(&Settings::default()),
            config.roots()[1].configurations()[1].cmake_configure_signature(&Settings::default())
        );
    }

    #[test]
    fn cmake_configures_with_the_generator_from_the_settings() {
        let mut config = two_roots();
        config.select_target(1, 1);
        let project = Path::new("/proj");
        let configure_args = |config: &BuildConfig, settings: &Settings| {
            let job = config.build_job(project, true, settings).unwrap();
            args(&job.steps[0].command)
        };
        let generator = |args: &[String]| {
            args.iter()
                .position(|a| a == "-G")
                .map(|i| args[i + 1].clone())
        };

        // Ninja unless set.
        let settings = Settings::default();
        let args = configure_args(&config, &settings);
        assert_eq!(generator(&args), Some("Ninja".to_owned()));
        assert_eq!(
            args[..4],
            ["-S", "/proj/native", "-B", "/proj/native/cmake-build-debug"]
        );
        assert_eq!(args[4..], ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Debug"]);
        let default_signature =
            config.roots()[1].configurations()[0].cmake_configure_signature(&settings);

        let mut settings = Settings::default();
        settings
            .set_text(SettingKey::CMakeGenerator, "Unix Makefiles")
            .unwrap();
        let args = configure_args(&config, &settings);
        assert_eq!(generator(&args), Some("Unix Makefiles".to_owned()));
        assert_ne!(
            config.roots()[1].configurations()[0].cmake_configure_signature(&settings),
            default_signature,
            "a new generator means configuring again"
        );

        // A blank setting leaves the choice to CMake.
        settings.set_text(SettingKey::CMakeGenerator, "").unwrap();
        let args = configure_args(&config, &settings);
        assert_eq!(generator(&args), None);
        assert!(!args.iter().any(|a| a.starts_with("-G")), "{args:?}");

        // The configuration's own -G, spaced or joined, wins over the
        // setting's, and is left where the user put it.
        settings
            .set_text(SettingKey::CMakeGenerator, "Xcode")
            .unwrap();
        for own in ["-G Ninja", "-GNinja", "-DX=1 -G 'Unix Makefiles'"] {
            config
                .set_configuration_text(1, 0, ConfigurationKey::ConfigureArgs, own)
                .unwrap();
            let args = configure_args(&config, &settings);
            assert_eq!(
                args.iter().filter(|a| a.starts_with("-G")).count(),
                1,
                "{own}: {args:?}"
            );
            assert!(!args.iter().any(|a| a == "Xcode"), "{own}: {args:?}");
        }
        assert_eq!(
            config.roots()[1].configurations()[0].cmake_generator(&settings),
            None
        );
        config
            .set_configuration_text(1, 0, ConfigurationKey::ConfigureArgs, "-DGEN=1")
            .unwrap();
        assert_eq!(
            config.roots()[1].configurations()[0].cmake_generator(&settings),
            Some("Xcode".to_owned()),
            "-D isn't -G"
        );
    }

    #[test]
    fn discovered_targets_follow_the_project_and_the_file_keeps_only_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\", \"c\"]\n",
        );
        for name in ["a", "b", "c"] {
            write(
                &root.join(name).join("Cargo.toml"),
                &format!("[package]\nname = \"{name}\"\n"),
            );
        }
        let mut config = BuildConfig::discover(root);
        let names = |config: &BuildConfig| -> Vec<(String, bool, bool)> {
            config.roots()[0]
                .targets()
                .iter()
                .map(|t| (t.name.clone(), t.is_auto(), t.disabled))
                .collect()
        };
        assert_eq!(
            names(&config),
            vec![
                ("a".to_owned(), true, false),
                ("b".to_owned(), true, false),
                ("c".to_owned(), true, false)
            ]
        );
        assert_eq!(config.roots()[0].targets()[0].auto.as_deref(), Some("a"));
        assert_eq!(
            config.roots()[0].discovered_as(&config.roots()[0].targets()[0]),
            Some("the package a".to_owned())
        );
        // Untouched, none of them is in the file.
        let text = config.to_toml();
        assert!(!text.contains("[[roots.targets]]"), "{text}");

        // Change one, disable another, add one of the user's own.
        config
            .set_target_text(0, 1, TargetKey::Arguments, "--fast")
            .unwrap();
        config.select_target(0, 2);
        assert!(config.set_target_disabled(0, 2, true));
        assert!(!config.set_target_disabled(0, 2, true), "already");
        assert_eq!(
            config.current().unwrap().target,
            0,
            "the selection leaves a disabled target"
        );
        config.select_target(0, 2);
        assert!(
            config
                .build_job(root, false, &Settings::default())
                .unwrap_err()
                .contains("c is disabled")
        );
        config.select_target(0, 0);
        assert_eq!(config.add_target(0), Some(3));
        config
            .set_target_text(0, 3, TargetKey::Name, "mine")
            .unwrap();
        assert_eq!(
            config.remove_target(0, 0).unwrap_err(),
            "a was found in Cargo.toml, so it can only be disabled, not removed"
        );
        let text = config.to_toml();
        assert!(!text.contains("name = \"a\""), "untouched: {text}");
        assert!(text.contains("auto = \"b\""), "{text}");
        assert!(text.contains("args = \"--fast\""), "{text}");
        assert!(text.contains("auto = \"c\""), "{text}");
        assert!(text.contains("disabled = true"), "{text}");
        assert!(text.contains("name = \"mine\""), "{text}");

        // Read back and synced: the same, in discovery order with the
        // user's own last.
        let mut again = BuildConfig::parse(&text).unwrap();
        again.sync_discovered(root);
        assert_eq!(again, config);
        assert_eq!(
            names(&again),
            vec![
                ("a".to_owned(), true, false),
                ("b".to_owned(), true, false),
                ("c".to_owned(), true, true),
                ("mine".to_owned(), false, false)
            ]
        );

        // The project changes: `a` and `c` go, `d` and a package named
        // like the user's target come. The untouched and the disabled
        // ones go away, `b` keeps its change, `d` appears, and the
        // newcomer's name makes way for the user's.
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"b\", \"d\", \"mine\"]\n",
        );
        write(&root.join("d/Cargo.toml"), "[package]\nname = \"d\"\n");
        write(
            &root.join("mine/Cargo.toml"),
            "[package]\nname = \"mine\"\n",
        );
        config.select_target(0, 1);
        let mut again = BuildConfig::parse(&config.to_toml()).unwrap();
        again.sync_discovered(root);
        assert_eq!(
            names(&again),
            vec![
                ("b".to_owned(), true, false),
                ("d".to_owned(), true, false),
                ("mine 2".to_owned(), true, false),
                ("mine".to_owned(), false, false)
            ]
        );
        assert_eq!(again.roots()[0].targets()[0].arguments, "--fast");
        assert_eq!(again.current_target().unwrap().1.name, "b");
        // Then `b` goes too: its change is kept as a target of the
        // user's own, which can be removed.
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"d\", \"mine\"]\n",
        );
        let mut again = BuildConfig::parse(&again.to_toml()).unwrap();
        again.sync_discovered(root);
        assert_eq!(
            names(&again),
            vec![
                ("d".to_owned(), true, false),
                ("mine 2".to_owned(), true, false),
                ("b".to_owned(), false, false),
                ("mine".to_owned(), false, false)
            ]
        );
        assert_eq!(again.current_target().unwrap().1.name, "b");
        assert!(again.remove_target(0, 2).is_ok());
        // A copy of a discovered target is the user's own.
        assert_eq!(again.duplicate_target(0, 0), Some(1));
        assert!(!again.roots()[0].targets()[1].is_auto());
        assert!(again.remove_target(0, 1).is_ok());
    }

    #[test]
    fn lists_come_in_name_order_for_display() {
        let mut config = two_roots();
        config.add_target(0);
        config
            .set_target_text(0, 2, TargetKey::Name, "Alpha")
            .unwrap();
        config.add_configuration(0);
        let root = &config.roots()[0];
        assert_eq!(root.targets_by_name(), vec![2, 0, 1]);
        assert_eq!(root.configurations_by_name(), vec![0, 2, 1]);
        assert_eq!(
            sorted_by_name(["b", "A", "a", "B"].into_iter()),
            vec![1, 2, 0, 3]
        );
    }

    #[test]
    fn clean_jobs_delete_the_current_or_every_build_directory() {
        let mut config = two_roots();
        let project = Path::new("/proj");
        // Cargo: clean the profile, leaving the directory to Cargo.
        config.select_configuration(0, 1);
        let job = config.clean_job(project).unwrap();
        assert_eq!(job.title, "Delete build directory (release)");
        assert_eq!(job.steps.len(), 1);
        let step = &job.steps[0];
        assert_eq!(step.description, "Cleaning Cargo.toml (release)");
        assert_eq!(step.command.program(), "cargo");
        assert_eq!(
            args(&step.command),
            vec!["clean", "--release", "--manifest-path", "/proj/Cargo.toml"]
        );
        assert_eq!(step.command.current_dir_path(), Some(project));

        // CMake: delete the named directory with CMake's own rm.
        config.select_configuration(1, 0);
        let job = config.clean_job(project).unwrap();
        assert_eq!(job.title, "Delete build directory (Debug)");
        let step = &job.steps[0];
        assert_eq!(step.description, "Deleting /proj/native/cmake-build-debug");
        assert_eq!(step.command.program(), "cmake");
        assert_eq!(
            args(&step.command),
            vec!["-E", "rm", "-rf", "/proj/native/cmake-build-debug"]
        );
        assert_eq!(
            step.command.current_dir_path(),
            Some(Path::new("/proj/native"))
        );

        // Everything: the Cargo root's whole target directory and each
        // CMake configuration's directory.
        let job = config.clean_all_job(project).unwrap();
        assert_eq!(job.title, "Delete all build directories");
        let commands: Vec<Vec<String>> = job.steps.iter().map(|s| args(&s.command)).collect();
        assert_eq!(
            commands,
            vec![
                vec!["clean", "--manifest-path", "/proj/Cargo.toml"],
                vec!["-E", "rm", "-rf", "/proj/native/cmake-build-debug"],
                vec!["-E", "rm", "-rf", "/proj/native/cmake-build-release"],
            ]
        );

        let empty = BuildConfig::default();
        assert_eq!(
            empty.clean_job(project).unwrap_err(),
            "No build configuration selected"
        );
        assert_eq!(
            empty.clean_all_job(project).unwrap_err(),
            "No build roots to clean"
        );
    }

    #[test]
    fn jobs_need_a_root_a_configuration_and_a_target() {
        let mut config = BuildConfig::default();
        let project = Path::new("/proj");
        assert!(
            config
                .build_job(project, true, &Settings::default())
                .unwrap_err()
                .contains("No build root")
        );
        config
            .add_root(BuildRoot::new(BuildSystem::Cargo, "Cargo.toml"))
            .unwrap();
        assert_eq!(
            config
                .build_job(project, true, &Settings::default())
                .unwrap_err(),
            "Cargo.toml has no configuration to build with"
        );
        config.add_configuration(0);
        assert_eq!(
            config
                .build_job(project, true, &Settings::default())
                .unwrap_err(),
            "Cargo.toml has no target to build"
        );
        config.add_target(0);
        assert_eq!(
            args(
                &config
                    .build_job(project, true, &Settings::default())
                    .unwrap()
                    .steps[0]
                    .command
            ),
            vec!["build"]
        );
    }

    #[test]
    fn file_round_trips_and_keeps_the_selection_by_name() {
        let mut config = two_roots();
        config.select_target(1, 1);
        config
            .set_configuration_text(1, 1, ConfigurationKey::ConfigureArgs, "-DX=1")
            .unwrap();
        let text = config.to_toml();
        assert!(text.starts_with(FILE_HEADER), "{text}");
        assert!(text.contains("[current]"), "{text}");
        assert!(text.contains("root = \"native/CMakeLists.txt\""), "{text}");
        assert!(text.contains("configuration = \"Debug\""), "{text}");
        assert!(text.contains("target = \"tool\""), "{text}");
        assert!(text.contains("[[roots]]"), "{text}");
        assert!(text.contains("[[roots.configurations]]"), "{text}");
        assert!(text.contains("configure-args = \"-DX=1\""), "{text}");
        assert!(
            !text.contains("build-args"),
            "blank options are left out: {text}"
        );
        let again = BuildConfig::parse(&text).unwrap();
        assert_eq!(again, config);

        // The selection is by name, so reordering the file keeps it.
        let reordered = text.replace("configuration = \"Debug\"", "configuration = \"Release\"");
        let again = BuildConfig::parse(&reordered).unwrap();
        assert_eq!(again.current().unwrap().configuration, 1);
        // Names that aren't there fall back to the first.
        let stale = text.replace("target = \"tool\"", "target = \"gone\"");
        assert_eq!(
            BuildConfig::parse(&stale)
                .unwrap()
                .current()
                .unwrap()
                .target,
            0
        );

        // An empty file is a configuration with no roots.
        let empty = BuildConfig::parse("").unwrap();
        assert!(empty.roots().is_empty());
        assert_eq!(empty.current(), None);
        assert_eq!(
            BuildConfig::parse(&BuildConfig::default().to_toml()).unwrap(),
            BuildConfig::default()
        );
    }

    #[test]
    fn a_root_pending_discovery_waits_for_the_target_the_file_named() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\"]\n",
        );
        write(&root.join("a/Cargo.toml"), "[package]\nname = \"a\"\n");
        write(&root.join("b/Cargo.toml"), "[package]\nname = \"b\"\n");
        let mut config = BuildConfig::discover(root);
        assert!(config.select_target(0, 1));
        let text = config.to_toml();
        assert!(text.contains("target = \"b\""), "{text}");

        // Loaded again, the file's target isn't among the targets yet:
        // the name is kept, shown as current, and written back as it
        // was, and a build waits rather than building the stand-in.
        let mut config = BuildConfig::parse(&text).unwrap();
        let discoveries = config.request_discovery(root);
        assert_eq!(discoveries.len(), 1);
        assert_eq!(discoveries[0].path(), Path::new("Cargo.toml"));
        assert!(config.is_discovering());
        assert!(config.roots()[0].is_pending());
        assert!(config.roots()[0].targets().is_empty());
        assert_eq!(config.pending_target(), Some("b"));
        let err = config.current_for_job().unwrap_err();
        assert!(err.contains("Still finding") && err.contains('b'), "{err}");
        assert!(config.build_job(root, false, &Settings::default()).is_err());
        assert!(config.to_toml().contains("target = \"b\""));
        // Then it turns up.
        let result = discoveries.into_iter().next().unwrap().run(None);
        assert_eq!(config.apply_discovery(result), Some(0));
        assert!(!config.is_discovering());
        assert_eq!(config.pending_target(), None);
        assert_eq!(config.current_target().unwrap().1.name, "b");
        assert!(config.current_for_job().is_ok());

        // A target picked meanwhile outranks the name from the file,
        // and stays current by name as the found ones go ahead of it.
        let mut config = BuildConfig::parse(&text).unwrap();
        let discoveries = config.request_discovery(root);
        assert_eq!(config.add_target(0), Some(0));
        // Already the stand-in, so nothing moves; but it is chosen now.
        assert!(!config.select_target(0, 0));
        assert_eq!(config.pending_target(), None);
        assert!(config.current_for_job().is_ok());
        config.apply_discovery(discoveries.into_iter().next().unwrap().run(None));
        assert_eq!(config.current().unwrap().target, 2);
        assert_eq!(config.current_target().unwrap().1.name, "New target");

        // A result for a root that is gone is dropped.
        let mut config = BuildConfig::parse(&text).unwrap();
        let discoveries = config.request_discovery(root);
        config.remove_root(0);
        let result = discoveries.into_iter().next().unwrap().run(None);
        assert_eq!(config.apply_discovery(result), None);
    }

    #[test]
    fn rejects_bad_files() {
        let err = |text: &str| BuildConfig::parse(text).unwrap_err().to_string();
        assert_eq!(err("roots = 1\n"), "`roots` must be an array of tables");
        assert_eq!(err("roots = [1]\n"), "`roots[0]` must be a table");
        assert_eq!(
            err("[[roots]]\npath = \"x\"\n"),
            "`roots[0]` has no `system`"
        );
        assert_eq!(
            err("[[roots]]\nsystem = \"make\"\npath = \"x\"\n"),
            "`roots[0].system` must be cargo or cmake"
        );
        assert_eq!(
            err("[[roots]]\nsystem = \"cargo\"\n"),
            "`roots[0]` has no `path`"
        );
        assert_eq!(
            err("[[roots]]\nsystem = \"cargo\"\npath = \"Cargo.toml\"\ntargets = 3\n"),
            "`roots[0].targets` must be an array of tables"
        );
        assert_eq!(
            err(
                "[[roots]]\nsystem = \"cargo\"\npath = \"Cargo.toml\"\n[[roots.configurations]]\nname = 1\n"
            ),
            "`roots[0].configurations[0].name` must be a string"
        );
        assert_eq!(
            err(
                "[[roots]]\nsystem = \"cargo\"\npath = \"Cargo.toml\"\n[[roots.targets]]\ndisabled = 1\n"
            ),
            "`roots[0].targets[0].disabled` must be true or false"
        );
        assert!(err("[[roots").contains("expected"));
        // Unknown keys are ignored, and nameless entries get names.
        let config = BuildConfig::parse(
            "future = true\n[[roots]]\nsystem = \"cmake\"\npath = \"CMakeLists.txt\"\ncolor = \"red\"\n[[roots.targets]]\nexecutable = \"a\"\n[[roots.targets]]\nexecutable = \"b\"\n",
        )
        .unwrap();
        let names: Vec<&str> = config.roots()[0]
            .targets()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["Unnamed", "Unnamed 2"]);
    }
}
