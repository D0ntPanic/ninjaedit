//! Reads a Python distribution's core metadata (a wheel's `METADATA`, served beside it as
//! `<wheel>.metadata`, or an sdist's `PKG-INFO`) and decides the project's license from it.
//!
//! A license can be declared three ways: a `License-Expression` in SPDX syntax (metadata 2.4),
//! trove classifiers (`License :: OSI Approved :: MIT License`), and the free-form `License`
//! field, which holds anything from an SPDX id to a license name to the whole license text. An
//! expression is authoritative; otherwise the classifiers and the field are all considered,
//! and where they disagree the most restrictive applies.

use super::{Declared, Source};
use crate::license::{self, Tier};

#[derive(Default)]
pub struct Metadata {
    pub name: Option<String>,
    pub version: Option<String>,
    pub license_expression: Option<String>,
    pub license: Option<String>,
    pub classifiers: Vec<String>,
    /// Names of the distributions required outside of extras.
    pub requires: Vec<String>,
}

/// Parses the header fields of a metadata file. Fields end at the first empty line, which
/// starts the long description; a line that opens with whitespace continues the field before.
pub fn parse(text: &str) -> Metadata {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        // Only an empty line ends the fields: a blank line inside a multi-line field is
        // written indented.
        if line.is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = fields.last_mut() {
                // setuptools indents the lines of a multi-line field by eight spaces, and
                // older versions also mark them with `|`.
                let rest = line.trim_start();
                value.push('\n');
                value.push_str(rest.strip_prefix('|').unwrap_or(rest));
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            fields.push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let mut meta = Metadata::default();
    for (key, value) in fields {
        let value = value.trim().to_owned();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "name" => meta.name = Some(value),
            "version" => meta.version = Some(value),
            "license-expression" => meta.license_expression = Some(value),
            "license" => meta.license = Some(value),
            "classifier" => meta.classifiers.push(value),
            "requires-dist" => {
                // `name[extra] (>=1.0) ; marker`: only unconditional or environment-dependent
                // requirements, not those of extras.
                let (spec, marker) = value.split_once(';').unwrap_or((&value, ""));
                if marker.contains("extra") {
                    continue;
                }
                let name: String = spec
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                    .collect();
                if !name.is_empty() && !meta.requires.contains(&name) {
                    meta.requires.push(name);
                }
            }
            _ => {}
        }
    }
    meta
}

/// The license the metadata declares, or `None` if it says nothing recognizable.
pub fn declared(meta: &Metadata) -> Option<Declared> {
    if let Some(expr) = &meta.license_expression
        && let Some(tier) = license::evaluate(expr, license::tier_of)
    {
        return Some(Declared {
            tier,
            source: Source::Expression,
            name: expr.clone(),
        });
    }
    let classifiers = meta
        .classifiers
        .iter()
        .filter_map(|c| classifier(c))
        .map(|(tier, name)| (tier, Source::Classifier, name));
    let field = meta
        .license
        .as_deref()
        .and_then(license_field)
        .map(|(tier, name)| (tier, Source::LicenseField, name));
    classifiers
        .chain(field)
        .min_by_key(|&(tier, _, _)| tier)
        .map(|(tier, source, name)| Declared { tier, source, name })
}

/// Rates a license trove classifier. The bare `OSI Approved` says nothing about which license;
/// a classifier that names one this does not know is not open source as far as the corpus is
/// concerned (`Other/Proprietary License`, `Free for non-commercial use`, `Freeware`).
fn classifier(classifier: &str) -> Option<(Tier, String)> {
    let rest = classifier.strip_prefix("License ::")?;
    let name = rest.rsplit("::").next()?.trim();
    if name.is_empty() || name == "OSI Approved" || name == "DFSG approved" {
        return None;
    }
    Some(license_name(name).unwrap_or((Tier::Other, name.to_owned())))
}

/// Rates the free-form `License` field: an SPDX expression, a license's name, or its text.
/// `UNKNOWN`, a file name and the like give `None`.
fn license_field(value: &str) -> Option<(Tier, String)> {
    let value = value.trim();
    if value.contains('\n') || value.len() > 200 {
        let first = value.lines().next().unwrap_or("");
        return license::from_header(value).or_else(|| license_name(first));
    }
    license::evaluate(value, license::tier_of)
        .filter(|&tier| tier != Tier::Other)
        .map(|tier| (tier, value.to_owned()))
        .or_else(|| license_name(value))
}

/// Words that name a license in the `License` field and in classifiers, with the tier and
/// the name to report. A word also matches followed by a version (`gplv3`, `apache2`).
const NAME_WORDS: &[(&str, Tier, &str)] = &[
    ("mit", Tier::Permissive, "MIT"),
    ("expat", Tier::Permissive, "MIT"),
    ("apache", Tier::Permissive, "Apache"),
    ("asl", Tier::Permissive, "Apache"),
    ("bsd", Tier::Permissive, "BSD"),
    ("0bsd", Tier::Permissive, "0BSD"),
    ("isc", Tier::Permissive, "ISC"),
    ("iscl", Tier::Permissive, "ISC"),
    ("unlicense", Tier::Permissive, "Unlicense"),
    ("cc0", Tier::Permissive, "CC0"),
    ("zlib", Tier::Permissive, "Zlib"),
    ("psf", Tier::Permissive, "PSF"),
    ("psfl", Tier::Permissive, "PSF"),
    ("cnri", Tier::Permissive, "PSF"),
    ("wtfpl", Tier::Permissive, "WTFPL"),
    ("boost", Tier::Permissive, "BSL-1.0"),
    ("bsl", Tier::Permissive, "BSL-1.0"),
    ("zope", Tier::Permissive, "ZPL"),
    ("zpl", Tier::Permissive, "ZPL"),
    ("hpnd", Tier::Permissive, "HPND"),
    ("upl", Tier::Permissive, "UPL"),
    ("afl", Tier::Permissive, "AFL"),
    ("postgresql", Tier::Permissive, "PostgreSQL"),
    ("blueoak", Tier::Permissive, "BlueOak"),
    ("ncsa", Tier::Permissive, "NCSA"),
    ("lgpl", Tier::WeakCopyleft, "LGPL"),
    ("mpl", Tier::WeakCopyleft, "MPL"),
    ("epl", Tier::WeakCopyleft, "EPL"),
    ("cddl", Tier::WeakCopyleft, "CDDL"),
    ("artistic", Tier::WeakCopyleft, "Artistic"),
    ("gpl", Tier::Copyleft, "GPL"),
    ("agpl", Tier::Copyleft, "AGPL"),
    ("eupl", Tier::Copyleft, "EUPL"),
    ("osl", Tier::Copyleft, "OSL"),
    ("sspl", Tier::Copyleft, "SSPL"),
    ("fdl", Tier::Copyleft, "GFDL"),
    ("gfdl", Tier::Copyleft, "GFDL"),
    ("cecill", Tier::Copyleft, "CeCILL"),
    ("proprietary", Tier::Other, "proprietary"),
    ("commercial", Tier::Other, "proprietary"),
    ("freeware", Tier::Other, "freeware"),
];

/// Phrases, after `license::words` normalization, that name a license beyond what
/// `license::from_text` recognizes.
const NAME_PHRASES: &[(&str, Tier, &str)] = &[
    ("public domain", Tier::Permissive, "public-domain"),
    ("all rights reserved", Tier::Other, "proprietary"),
    ("bsd 4 clause", Tier::Other, "BSD-4-clause"),
    ("original bsd", Tier::Other, "BSD-4-clause"),
    ("cc by", Tier::Permissive, "CC-BY"),
    ("creative commons attribution", Tier::Permissive, "CC-BY"),
];

/// Clauses that make a Creative Commons license more than attribution.
const CC_CLAUSES: &[&str] = &[
    " sa ",
    " nc ",
    " nd ",
    " sharealike ",
    " share alike ",
    " noncommercial ",
    " non commercial ",
    " noderivatives ",
    " no derivatives ",
];

/// Rates a license's name, as people write it. Several names are alternatives when the text
/// says so ("MIT or Apache-2.0", "dual licensed"), and otherwise all apply.
fn license_name(name: &str) -> Option<(Tier, String)> {
    let text = license::words(name);
    let mut found: Vec<(Tier, &str)> = license::from_text(name);
    let mut add = |item: (Tier, &'static str)| {
        if !found.contains(&item) {
            found.push(item);
        }
    };
    for &(phrase, tier, name) in NAME_PHRASES {
        if text.contains(&format!(" {phrase} ")) {
            let restricted = name == "CC-BY" && CC_CLAUSES.iter().any(|c| text.contains(c));
            add(if restricted {
                (Tier::Other, "CC")
            } else {
                (tier, name)
            });
        }
    }
    for word in text.split_whitespace() {
        for &(key, tier, name) in NAME_WORDS {
            let versioned = word.strip_prefix(key).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with(|c: char| c == 'v' || c.is_ascii_digit())
            });
            if versioned {
                add((tier, name));
            }
        }
    }
    // The BSD family word also matched a 4-clause BSD named as such.
    if found.contains(&(Tier::Other, "BSD-4-clause")) {
        found.retain(|&(_, n)| n != "BSD");
    }
    let alternatives = [" or ", " dual ", " either "]
        .iter()
        .any(|w| text.contains(w));
    let tier = if alternatives {
        found.iter().map(|&(t, _)| t).max()
    } else {
        found.iter().map(|&(t, _)| t).min()
    }?;
    let names: Vec<&str> = found.iter().map(|&(_, n)| n).collect();
    Some((tier, names.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(field: &str) -> Option<Tier> {
        license_field(field).map(|(t, _)| t)
    }

    #[test]
    fn fields() {
        let meta = parse(
            "Metadata-Version: 2.1\r\nName: chopro-epub\r\nVersion: 1.1.1\r\nLicense: MIT License\r\n        \r\n        Copyright (c) 2020\r\nClassifier: License :: OSI Approved :: MIT License\r\nRequires-Dist: pyparsing\r\nRequires-Dist: importlib-resources (>=1.1.0) ; python_version < \"3.9\"\r\nRequires-Dist: pytest ; extra == 'dev'\r\n\r\nLicense: not a field\n",
        );
        assert_eq!(meta.name.as_deref(), Some("chopro-epub"));
        assert_eq!(meta.version.as_deref(), Some("1.1.1"));
        assert_eq!(
            meta.license.as_deref(),
            Some("MIT License\n\nCopyright (c) 2020")
        );
        assert_eq!(meta.classifiers, ["License :: OSI Approved :: MIT License"]);
        assert_eq!(meta.requires, ["pyparsing", "importlib-resources"]);
    }

    #[test]
    fn license_fields() {
        assert_eq!(tier("MIT"), Some(Tier::Permissive));
        assert_eq!(tier("MIT License"), Some(Tier::Permissive));
        assert_eq!(tier("The MIT License (MIT)"), Some(Tier::Permissive));
        assert_eq!(tier("Apache License, Version 2.0"), Some(Tier::Permissive));
        assert_eq!(tier("Apache 2.0"), Some(Tier::Permissive));
        assert_eq!(tier("BSD"), Some(Tier::Permissive));
        assert_eq!(tier("BSD 3-Clause License"), Some(Tier::Permissive));
        assert_eq!(tier("MIT OR Apache-2.0"), Some(Tier::Permissive));
        assert_eq!(tier("GPLv3"), Some(Tier::Copyleft));
        assert_eq!(tier("GPLv3+"), Some(Tier::Copyleft));
        assert_eq!(tier("AGPL-3"), Some(Tier::Copyleft));
        assert_eq!(tier("GNU GENERAL PUBLIC LICENSE"), Some(Tier::Copyleft));
        assert_eq!(tier("LGPLv3"), Some(Tier::WeakCopyleft));
        assert_eq!(tier("MIT and GPL"), Some(Tier::Copyleft));
        assert_eq!(
            tier("Dual licensed under MIT or GPL"),
            Some(Tier::Permissive)
        );
        assert_eq!(tier("Proprietary"), Some(Tier::Other));
        assert_eq!(tier("BSD 4-clause"), Some(Tier::Other));
        assert_eq!(tier("Public Domain"), Some(Tier::Permissive));
        assert_eq!(
            tier("University of Illinois/NCSA Open Source License"),
            Some(Tier::Permissive)
        );
        assert_eq!(tier("CC BY 4.0"), Some(Tier::Permissive));
        assert_eq!(tier("CC BY-SA 4.0"), Some(Tier::Other));
        assert_eq!(
            tier("Creative Commons Attribution-NonCommercial 4.0"),
            Some(Tier::Other)
        );
        assert_eq!(tier("UNKNOWN"), None);
        assert_eq!(tier("LICENSE.txt"), None);
        assert_eq!(tier("Unlicensed"), None);
        assert_eq!(tier("Commit to the mitigation"), None);
        let text = "MIT License\n\nCopyright (c) 2021 Someone\n\nPermission is hereby granted, free of charge, to any person obtaining a copy";
        assert_eq!(tier(text), Some(Tier::Permissive));
    }

    #[test]
    fn classifiers() {
        let rate = |c: &str| classifier(c).map(|(t, _)| t);
        assert_eq!(
            rate("License :: OSI Approved :: MIT License"),
            Some(Tier::Permissive)
        );
        assert_eq!(
            rate("License :: OSI Approved :: Apache Software License"),
            Some(Tier::Permissive)
        );
        assert_eq!(
            rate("License :: OSI Approved :: GNU Lesser General Public License v3 (LGPLv3)"),
            Some(Tier::WeakCopyleft)
        );
        assert_eq!(
            rate("License :: OSI Approved :: GNU Affero General Public License v3"),
            Some(Tier::Copyleft)
        );
        assert_eq!(
            rate("License :: OSI Approved :: Mozilla Public License 2.0 (MPL 2.0)"),
            Some(Tier::WeakCopyleft)
        );
        assert_eq!(
            rate("License :: CC0 1.0 Universal (CC0 1.0) Public Domain Dedication"),
            Some(Tier::Permissive)
        );
        assert_eq!(rate("License :: Public Domain"), Some(Tier::Permissive));
        assert_eq!(
            rate("License :: Other/Proprietary License"),
            Some(Tier::Other)
        );
        assert_eq!(
            rate("License :: Free for non-commercial use"),
            Some(Tier::Other)
        );
        assert_eq!(rate("License :: Freely Distributable"), Some(Tier::Other));
        assert_eq!(rate("License :: OSI Approved"), None);
        assert_eq!(rate("Programming Language :: Python"), None);
    }

    #[test]
    fn declarations() {
        let meta = |expr: Option<&str>, license: Option<&str>, classifiers: &[&str]| Metadata {
            license_expression: expr.map(str::to_owned),
            license: license.map(str::to_owned),
            classifiers: classifiers.iter().map(|c| c.to_string()).collect(),
            ..Metadata::default()
        };
        let mit = "License :: OSI Approved :: MIT License";
        let gpl = "License :: OSI Approved :: GNU General Public License v3 (GPLv3)";
        // An expression is authoritative.
        let d = declared(&meta(Some("MIT"), Some("GPL"), &[gpl])).unwrap();
        assert_eq!((d.tier, d.source), (Tier::Permissive, Source::Expression));
        // Otherwise the most restrictive declaration applies.
        let d = declared(&meta(None, Some("MIT"), &[gpl])).unwrap();
        assert_eq!((d.tier, d.source), (Tier::Copyleft, Source::Classifier));
        let d = declared(&meta(None, Some("UNKNOWN"), &[mit])).unwrap();
        assert_eq!((d.tier, d.source), (Tier::Permissive, Source::Classifier));
        let d = declared(&meta(None, Some("Apache 2.0"), &[])).unwrap();
        assert_eq!((d.tier, d.source), (Tier::Permissive, Source::LicenseField));
        assert!(declared(&meta(None, Some("UNKNOWN"), &["License :: OSI Approved"])).is_none());
    }
}
