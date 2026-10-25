//! SSH config files: `~/.ssh/config`, the files in `~/.ssh/config.d/`, and the system's
//! `/etc/ssh/ssh_config` and `sshd_config` with their `.d` folders. Which files they are (none has
//! a telling extension, so the folder names say), which parts of a line to colour, whether every
//! line is well formed (a keyword ssh knows, with a value), and the `Host` block the command
//! palette puts in.

use std::ops::Range;
use std::path::Path;

use crate::core::syntax::{Found, Token, indent, quoted_end};

/// Whether `path` is an SSH config file, by the folders it sits in: `config` in a `.ssh` folder,
/// anything in `.ssh/config.d/`, or `ssh_config`/`sshd_config` (and their `.d` folders) in an
/// `ssh` folder such as `/etc/ssh`. Folder names, not whole paths, so the same test holds on
/// Linux, macOS and Windows.
pub fn is_ssh_config(path: &Path) -> bool {
    let file = name(Some(path));
    let folder = name(path.parent());
    let above = name(path.parent().and_then(Path::parent));
    matches!(
        (above, folder, file),
        (_, Some(".ssh"), Some("config"))
            | (Some(".ssh"), Some("config.d"), Some(_))
            | (_, Some("ssh"), Some("ssh_config" | "sshd_config"))
            | (Some("ssh"), Some("ssh_config.d" | "sshd_config.d"), Some(_))
    )
}

/// The last part of `path`, when there is one and it's text.
fn name(path: Option<&Path>) -> Option<&str> {
    path.and_then(Path::file_name)
        .and_then(|name| name.to_str())
}

/// Colours one line: a `#` comment whole; `Host` and `Match` as sections (they start a block);
/// any other keyword as a key; then each argument, quoted ones as strings, numbers, `yes`, `no`
/// and ssh's other fixed words (`ask`, `none`, ...) as literals, and the rest as strings.
pub fn ssh_ranges(line: &str) -> Found {
    let start = indent(line);
    let rest = line[start..].trim_end();
    let end = start + rest.len();
    if rest.starts_with('#') {
        return vec![(start..end, Token::Comment)];
    }
    let Some((keyword, mut at)) = word(line, start, end) else {
        return Vec::new();
    };
    let token = match line[keyword.clone()].to_ascii_lowercase().as_str() {
        "host" | "match" => Token::Section,
        _ => Token::Key,
    };
    let mut found = vec![(keyword, token)];
    // `Key value` or `Key=value`.
    if line[at..end].trim_start().starts_with('=') {
        at += line[at..end].find('=').unwrap_or(0) + 1;
    }
    while let Some((argument, after)) = word(line, at, end) {
        let token = match &line[argument.clone()] {
            quoted if quoted.starts_with('"') => Token::String,
            digits if digits.bytes().all(|byte| byte.is_ascii_digit()) => Token::Number,
            word => match word.to_ascii_lowercase().as_str() {
                "yes" | "no" | "ask" | "none" | "any" | "always" | "auto" | "autoask"
                | "confirm" | "accept-new" | "all" | "canonical" | "final" => Token::Literal,
                _ => Token::String,
            },
        };
        found.push((argument, token));
        at = after;
    }
    found
}

/// The next word of `line[..end]` at or after `at` (a `"quoted"` one whole), and where it ends,
/// or `None` when only spaces are left. `=` is a word of its own, so `Port=22` splits in three.
fn word(line: &str, at: usize, end: usize) -> Option<(Range<usize>, usize)> {
    let rest = &line[at..end];
    let start = at + (rest.len() - rest.trim_start().len());
    if start == end {
        return None;
    }
    let stop = match line.as_bytes()[start] {
        b'"' => quoted_end(line, start),
        b'=' => start + 1,
        _ => line[start..end]
            .find([' ', '\t', '='])
            .map_or(end, |found| start + found),
    };
    Some((start..stop, stop))
}

/// Every keyword OpenSSH's ssh_config and sshd_config know (as of OpenSSH 9.9, plus the GSSAPI
/// key exchange ones Debian and Red Hat patch in), in the man pages' casing. One list for both
/// files: ssh accepts them in any case, and a client option in sshd_config is rare enough that
/// telling the two apart isn't worth a second list. `ssh` refuses to run at all on a keyword it
/// doesn't know, so a typo here is worth a word.
pub const KEYWORDS: &[&str] = &[
    "AcceptEnv",
    "AddKeysToAgent",
    "AddressFamily",
    "AllowAgentForwarding",
    "AllowGroups",
    "AllowStreamLocalForwarding",
    "AllowTcpForwarding",
    "AllowUsers",
    "AuthenticationMethods",
    "AuthorizedKeysCommand",
    "AuthorizedKeysCommandUser",
    "AuthorizedKeysFile",
    "AuthorizedPrincipalsCommand",
    "AuthorizedPrincipalsCommandUser",
    "AuthorizedPrincipalsFile",
    "Banner",
    "BatchMode",
    "BindAddress",
    "BindInterface",
    "CanonicalDomains",
    "CanonicalizeFallbackLocal",
    "CanonicalizeHostname",
    "CanonicalizeMaxDots",
    "CanonicalizePermittedCNAMEs",
    "CASignatureAlgorithms",
    "CertificateFile",
    "ChallengeResponseAuthentication",
    "ChannelTimeout",
    "CheckHostIP",
    "ChrootDirectory",
    "Ciphers",
    "ClearAllForwardings",
    "ClientAliveCountMax",
    "ClientAliveInterval",
    "Compression",
    "ConnectionAttempts",
    "ConnectTimeout",
    "ControlMaster",
    "ControlPath",
    "ControlPersist",
    "DenyGroups",
    "DenyUsers",
    "DisableForwarding",
    "DynamicForward",
    "EnableEscapeCommandline",
    "EnableSSHKeysign",
    "EscapeChar",
    "ExitOnForwardFailure",
    "ExposeAuthInfo",
    "FingerprintHash",
    "ForceCommand",
    "ForkAfterAuthentication",
    "ForwardAgent",
    "ForwardX11",
    "ForwardX11Timeout",
    "ForwardX11Trusted",
    "GatewayPorts",
    "GlobalKnownHostsFile",
    "GSSAPIAuthentication",
    "GSSAPICleanupCredentials",
    "GSSAPIClientIdentity",
    "GSSAPIDelegateCredentials",
    "GSSAPIKexAlgorithms",
    "GSSAPIKeyExchange",
    "GSSAPIRenewalForcesRekey",
    "GSSAPIServerIdentity",
    "GSSAPIStrictAcceptorCheck",
    "GSSAPITrustDns",
    "HashKnownHosts",
    "Host",
    "HostbasedAcceptedAlgorithms",
    "HostbasedAuthentication",
    "HostbasedUsesNameFromPacketOnly",
    "HostCertificate",
    "HostKey",
    "HostKeyAgent",
    "HostKeyAlgorithms",
    "HostKeyAlias",
    "HostName",
    "IdentitiesOnly",
    "IdentityAgent",
    "IdentityFile",
    "IgnoreRhosts",
    "IgnoreUnknown",
    "IgnoreUserKnownHosts",
    "Include",
    "IPQoS",
    "KbdInteractiveAuthentication",
    "KbdInteractiveDevices",
    "KerberosAuthentication",
    "KerberosGetAFSToken",
    "KerberosOrLocalPasswd",
    "KerberosTicketCleanup",
    "KexAlgorithms",
    "KnownHostsCommand",
    "ListenAddress",
    "LocalCommand",
    "LocalForward",
    "LoginGraceTime",
    "LogLevel",
    "LogVerbose",
    "MACs",
    "Match",
    "MaxAuthTries",
    "MaxSessions",
    "MaxStartups",
    "ModuliFile",
    "NoHostAuthenticationForLocalhost",
    "NumberOfPasswordPrompts",
    "ObscureKeystrokeTiming",
    "PasswordAuthentication",
    "PermitEmptyPasswords",
    "PermitListen",
    "PermitLocalCommand",
    "PermitOpen",
    "PermitRemoteOpen",
    "PermitRootLogin",
    "PermitTTY",
    "PermitTunnel",
    "PermitUserEnvironment",
    "PermitUserRC",
    "PerSourceMaxStartups",
    "PerSourceNetBlockSize",
    "PerSourcePenalties",
    "PerSourcePenaltyExemptList",
    "PidFile",
    "PKCS11Provider",
    "Port",
    "PreferredAuthentications",
    "PrintLastLog",
    "PrintMotd",
    "Protocol",
    "ProxyCommand",
    "ProxyJump",
    "ProxyUseFdpass",
    "PubkeyAcceptedAlgorithms",
    "PubkeyAcceptedKeyTypes",
    "PubkeyAuthentication",
    "PubkeyAuthOptions",
    "RefuseConnection",
    "RekeyLimit",
    "RemoteCommand",
    "RemoteForward",
    "RequestTTY",
    "RequiredRSASize",
    "RevokedHostKeys",
    "RevokedKeys",
    "SecurityKeyProvider",
    "SendEnv",
    "ServerAliveCountMax",
    "ServerAliveInterval",
    "SessionType",
    "SetEnv",
    "StdinNull",
    "StreamLocalBindMask",
    "StreamLocalBindUnlink",
    "StrictHostKeyChecking",
    "StrictModes",
    "Subsystem",
    "SyslogFacility",
    "Tag",
    "TCPKeepAlive",
    "TrustedUserCAKeys",
    "Tunnel",
    "TunnelDevice",
    "UnusedConnectionTimeout",
    "UpdateHostKeys",
    "UseDNS",
    "UsePAM",
    "User",
    "UserKnownHostsFile",
    "VerifyHostKeyDNS",
    "VersionAddendum",
    "VisualHostKey",
    "X11DisplayOffset",
    "X11Forwarding",
    "X11UseLocalhost",
    "XAuthLocation",
];

/// Why `text` isn't a well-formed SSH config, with where, or `None` if it is. Each line must be
/// blank, a `#` comment, or a keyword ssh knows (`KEYWORDS`, in any case) with at least one
/// argument, and a `"` must close on its line.
pub fn ssh_problem(text: &str) -> Option<String> {
    for (index, line) in text.lines().enumerate() {
        let start = indent(line);
        let rest = line.trim();
        if rest.is_empty() || rest.starts_with('#') {
            continue;
        }
        let end = start + rest.len();
        let (keyword, mut after) = word(line, start, end)?;
        // `Key = value` is allowed too.
        if let Some((equals, past)) = word(line, after, end)
            && &line[equals] == "="
        {
            after = past;
        }
        let keyword = &line[keyword];
        let problem = if !keyword.chars().all(|c| c.is_ascii_alphabetic()) {
            "expected a keyword, such as Host or Port".to_string()
        } else if !KEYWORDS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(keyword))
        {
            format!("ssh doesn't know the keyword {keyword}")
        } else if word(line, after, end).is_none() {
            "the keyword needs a value".to_string()
        } else if line[start..end].match_indices('"').count() % 2 == 1 {
            "a \" isn't closed".to_string()
        } else {
            continue;
        };
        return Some(format!(
            "Invalid SSH config at line {}, column {}: {problem}",
            index + 1,
            start + 1
        ));
    }
    None
}

/// The palette's New Host block: where in `text` it goes (the end of the caret's line, kept apart
/// from its neighbours by one blank line), the text to put there (indented by `indent` spaces,
/// `User` set to `user`), and the byte range of the `alias` placeholder once it's in, to select so
/// typing names the host.
pub fn host_block(
    text: &str,
    caret: usize,
    indent: usize,
    user: &str,
) -> (usize, String, Range<usize>) {
    let at = text[caret..]
        .find('\n')
        .map_or(text.len(), |newline| caret + newline);
    let before = &text[..at];
    let blank_lines_before = before.len() - before.trim_end_matches('\n').len();
    let gap = match () {
        _ if before.is_empty() => 0,
        _ if before.trim_end_matches('\n').is_empty() => 0,
        _ => 2usize.saturating_sub(blank_lines_before),
    };
    // The newline already there ends the block's last line; a blank line is kept after it.
    let after = &text[at..];
    let close = match after {
        "" => "\n",
        _ if after.starts_with("\n\n") => "",
        _ => "\n",
    };
    let pad = " ".repeat(indent);
    let block = format!(
        "{}Host alias\n{pad}HostName alias.example.com\n{pad}User {user}\n{pad}Port 22\n{pad}IdentityFile ~/.ssh/id_ed25519{close}",
        "\n".repeat(gap)
    );
    let alias = at + gap + "Host ".len();
    (at, block, alias..alias + "alias".len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Token::*;

    #[test]
    fn ssh_config_files_are_known_by_their_folders() {
        let ssh = |path| is_ssh_config(Path::new(path));
        assert!(ssh("/home/me/.ssh/config"));
        assert!(
            ssh(r"C:\Users\me\.ssh\config") || cfg!(unix),
            "Windows paths split on \\ there"
        );
        assert!(ssh("/home/me/.ssh/config.d/work"));
        assert!(ssh("/etc/ssh/ssh_config"));
        assert!(ssh("/etc/ssh/sshd_config"));
        assert!(ssh("/etc/ssh/ssh_config.d/50-redhat.conf"));
        assert!(ssh("/etc/ssh/sshd_config.d/10-port.conf"));
        assert!(!ssh("/home/me/.ssh/known_hosts"));
        assert!(!ssh("/home/me/.ssh/id_ed25519"));
        assert!(!ssh("/home/me/config"));
        assert!(!ssh("/home/me/.ssh/other.d/config"));
        assert!(!ssh("config"));
    }

    fn looks(line: &str) -> Vec<(&str, Token)> {
        ssh_ranges(line)
            .into_iter()
            .map(|(range, look)| (&line[range], look))
            .collect()
    }

    #[test]
    fn host_lines_are_sections_and_options_are_keys_with_values() {
        assert_eq!(looks("# work"), [("# work", Comment)]);
        assert_eq!(looks("   "), []);
        assert_eq!(
            looks("Host dev *.example.com"),
            [
                ("Host", Section),
                ("dev", String),
                ("*.example.com", String)
            ]
        );
        assert_eq!(
            looks("Match host dev user me"),
            [
                ("Match", Section),
                ("host", String),
                ("dev", String),
                ("user", String),
                ("me", String)
            ]
        );
        assert_eq!(looks("    Port 22"), [("Port", Key), ("22", Number)]);
        assert_eq!(looks("Port=2222"), [("Port", Key), ("2222", Number)]);
        assert_eq!(looks("Port = 2222"), [("Port", Key), ("2222", Number)]);
        assert_eq!(
            looks("  ForwardAgent yes"),
            [("ForwardAgent", Key), ("yes", Literal)]
        );
        assert_eq!(
            looks("StrictHostKeyChecking accept-new"),
            [("StrictHostKeyChecking", Key), ("accept-new", Literal)]
        );
        assert_eq!(
            looks("IdentityFile \"~/my keys/id\" "),
            [("IdentityFile", Key), ("\"~/my keys/id\"", String)]
        );
        assert_eq!(
            looks("HostName 10.0.0.5"),
            [("HostName", Key), ("10.0.0.5", String)]
        );
        assert_eq!(looks("Compression"), [("Compression", Key)]);
    }

    #[test]
    fn every_line_needs_a_keyword_ssh_knows_and_a_value() {
        assert_eq!(
            ssh_problem("# all\n\nHost dev\n    Port 22\n    User me\n"),
            None
        );
        assert_eq!(
            ssh_problem("Port=22\nPort = 22\nhostname a\nHOSTNAME b"),
            None
        );
        assert_eq!(
            ssh_problem("Host dev\n    IdentityFiel ~/.ssh/id\n"),
            Some(
                "Invalid SSH config at line 2, column 5: ssh doesn't know the keyword IdentityFiel"
                    .into()
            )
        );
        assert!(
            KEYWORDS
                .windows(2)
                .all(|pair| pair[0].to_lowercase() < pair[1].to_lowercase()),
            "sorted, for reading"
        );
        assert_eq!(
            ssh_problem("Host dev\n    Port\n"),
            Some("Invalid SSH config at line 2, column 5: the keyword needs a value".into())
        );
        assert_eq!(
            ssh_problem("Port =\n"),
            Some("Invalid SSH config at line 1, column 1: the keyword needs a value".into())
        );
        assert_eq!(
            ssh_problem("Host dev\n[section]\n"),
            Some(
                "Invalid SSH config at line 2, column 1: expected a keyword, such as Host or Port"
                    .into()
            )
        );
        assert_eq!(
            ssh_problem("IdentityFile \"~/keys\n"),
            Some("Invalid SSH config at line 1, column 1: a \" isn't closed".into())
        );
    }

    #[test]
    fn a_new_host_block_goes_after_the_carets_line_with_a_blank_line_around() {
        let block = |text: &str, caret| {
            let (at, with, alias) = host_block(text, caret, 4, "me");
            let mut result = text.to_string();
            result.replace_range(at..at, &with);
            assert_eq!(&result[alias], "alias");
            result
        };
        let host = "Host alias\n    HostName alias.example.com\n    User me\n    Port 22\n    IdentityFile ~/.ssh/id_ed25519\n";
        assert_eq!(block("", 0), host);
        assert_eq!(block("\n\n", 0), host.to_string() + "\n");
        assert_eq!(
            block("Host a\n    Port 1\n", 3),
            format!("Host a\n\n{host}\n    Port 1\n")
        );
        assert_eq!(
            block("Host a\n    Port 1", 12),
            format!("Host a\n    Port 1\n\n{host}")
        );
        assert_eq!(
            block("Host a\n\nHost b\n", 7),
            format!("Host a\n\n{host}\nHost b\n")
        );
        // Extra blank lines around it aren't kept.
        assert_eq!(
            block("Host a\n\n\nHost b\n", 7),
            format!("Host a\n\n{host}\nHost b\n")
        );
        assert_eq!(block("Host a\n", 7), format!("Host a\n\n{host}"));
    }
}
