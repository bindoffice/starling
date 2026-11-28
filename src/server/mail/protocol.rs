use super::*;
#[derive(Debug, Clone)]
pub(super) struct SessionContext {
    pub remote: ListenAddress,
    pub local: ListenAddress,
    pub proxy_addresses: ProxyProtocolAddresses,
    pub client_host: Option<String>,
    pub tls: Option<TlsConnectionInfo>,
    pub client_certificate_verify: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct AuthContext {
    pub user: Vec<u8>,
    pub password: Vec<u8>,
    pub password_available: bool,
    pub method: MailAuthMethod,
    pub salt: Vec<u8>,
    pub smtp_helo: Vec<u8>,
    pub smtp_from: Vec<u8>,
    pub smtp_to: Vec<u8>,
    pub imap_tag: Vec<u8>,
    pub backend: Option<(String, u16)>,
}

pub(super) enum AuthOutcome {
    Authenticated(AuthContext),
    StartTls,
    Closed,
}

#[derive(Debug)]
enum AuthReply {
    Accepted(AuthHttpResponse),
    Rejected(AuthHttpResponse),
}

pub(super) async fn authenticate(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
) -> io::Result<AuthOutcome> {
    let timeout = server.timeout;
    let mut greeting = match server.protocol {
        MailProtocol::Pop3 => {
            let salt = required_salt(server, context);
            if salt.is_empty() {
                b"+OK POP3 ready\r\n".to_vec()
            } else {
                let mut greeting = b"+OK POP3 ready <".to_vec();
                greeting.extend_from_slice(salt.as_bytes());
                greeting.extend_from_slice(b">\r\n");
                greeting
            }
        }
        MailProtocol::Imap => b"* OK IMAP4 ready\r\n".to_vec(),
        MailProtocol::Smtp => {
            if !server.greeting_delay.is_zero()
                && smtp_reject_premature_command(reader, server).await?
            {
                return Ok(AuthOutcome::Closed);
            }
            format!("220 {} ESMTP ready\r\n", server.server_name).into_bytes()
        }
    };
    if server.protocol == MailProtocol::Pop3 {
        greeting.shrink_to_fit();
    }
    write_client(reader, &greeting, timeout).await?;

    match server.protocol {
        MailProtocol::Pop3 => authenticate_pop3(reader, server, context).await,
        MailProtocol::Imap => authenticate_imap(reader, server, context).await,
        MailProtocol::Smtp => authenticate_smtp(reader, server, context).await,
    }
}

async fn smtp_reject_premature_command(
    reader: &mut MailReader,
    server: &MailServer,
) -> io::Result<bool> {
    let delay = tokio::time::sleep(server.greeting_delay);
    tokio::pin!(delay);
    tokio::select! {
        _ = &mut delay => Ok(false),
        result = reader.fill_buf() => {
            match result {
                Ok([]) => Ok(true),
                Ok(_) => {
                    write_client(
                        reader,
                        b"503 5.5.0 Improper use of SMTP command pipelining\r\n",
                        server.timeout,
                    )
                    .await?;
                    Ok(true)
                }
                Err(error) => Err(error),
            }
        }
    }
}

fn required_salt(server: &MailServer, context: &SessionContext) -> String {
    if !server.has_auth_method(MailAuthMethod::Apop)
        && !server.has_auth_method(MailAuthMethod::CramMd5)
    {
        return String::new();
    }
    let remote = context
        .remote
        .socket_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|| "unix".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}.{}@{}",
        std::process::id(),
        nanos ^ hash_bytes(remote.as_bytes()) as u128,
        server.server_name
    )
}

fn hash_bytes(value: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

async fn authenticate_smtp(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
) -> io::Result<AuthOutcome> {
    let mut errors = 0_usize;
    let mut helo = Vec::new();
    let mut from = Vec::new();
    let mut to = Vec::new();
    loop {
        let line = read_line(reader, server.client_buffer_size, server.timeout).await?;
        let Some(line) = line else {
            return Ok(AuthOutcome::Closed);
        };
        let line = trim_line(&line);
        if line.is_empty() {
            continue;
        }
        let (command, rest) = split_command(line);
        if command.eq_ignore_ascii_case(b"QUIT") {
            write_client(reader, b"221 2.0.0 Bye\r\n", server.timeout).await?;
            return Ok(AuthOutcome::Closed);
        }
        if command.eq_ignore_ascii_case(b"NOOP") {
            write_client(reader, b"250 2.0.0 OK\r\n", server.timeout).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"RSET") {
            from.clear();
            to.clear();
            write_client(reader, b"250 2.0.0 OK\r\n", server.timeout).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"HELO") || command.eq_ignore_ascii_case(b"EHLO") {
            if rest.is_empty() {
                write_client(reader, b"501 5.5.4 Invalid argument\r\n", server.timeout).await?;
                continue;
            }
            helo = rest.to_vec();
            let esmtp = command.eq_ignore_ascii_case(b"EHLO");
            from.clear();
            to.clear();
            let response = smtp_helo_response(server, context, esmtp);
            write_client(reader, &response, server.timeout).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"STARTTLS") {
            if context.tls.is_none() && server.tls.starttls != MailStartTls::Off {
                write_client(reader, b"220 2.0.0 Start TLS\r\n", server.timeout).await?;
                return Ok(AuthOutcome::StartTls);
            }
            invalid_smtp(reader, server, &mut errors).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"AUTH") {
            if server.tls.starttls == MailStartTls::Only && context.tls.is_none() {
                invalid_smtp(reader, server, &mut errors).await?;
                continue;
            }
            let (method_value, auth_argument) = split_sasl_method(rest);
            let Some(method) = parse_auth_method(method_value) else {
                write_client(reader, b"501 5.5.4 Invalid argument\r\n", server.timeout).await?;
                continue;
            };
            if !authentication_method_enabled(server, method) {
                invalid_smtp(reader, server, &mut errors).await?;
                continue;
            }
            match smtp_authenticate_method(
                reader,
                server,
                context,
                method,
                auth_argument,
                helo.clone(),
                from.clone(),
                to.clone(),
            )
            .await?
            {
                Some(auth) => return Ok(AuthOutcome::Authenticated(auth)),
                None => continue,
            }
        }
        if command.eq_ignore_ascii_case(b"MAIL") {
            if !server.has_auth_method(MailAuthMethod::None) {
                write_client(
                    reader,
                    b"530 5.7.1 Authentication required\r\n",
                    server.timeout,
                )
                .await?;
                continue;
            }
            if !from.is_empty() {
                write_client(
                    reader,
                    b"503 5.5.1 Bad sequence of commands\r\n",
                    server.timeout,
                )
                .await?;
                continue;
            }
            if rest.is_empty() {
                write_client(reader, b"501 5.5.4 Invalid argument\r\n", server.timeout).await?;
                continue;
            }
            from.clear();
            from.extend_from_slice(b"MAIL ");
            from.extend_from_slice(rest);
            write_client(reader, b"250 2.0.0 OK\r\n", server.timeout).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"RCPT") {
            if from.is_empty() {
                write_client(
                    reader,
                    b"503 5.5.1 Bad sequence of commands\r\n",
                    server.timeout,
                )
                .await?;
                continue;
            }
            if rest.is_empty() {
                write_client(reader, b"501 5.5.4 Invalid argument\r\n", server.timeout).await?;
                continue;
            }
            to.clear();
            to.extend_from_slice(b"RCPT ");
            to.extend_from_slice(rest);
            let mut auth = AuthContext {
                user: Vec::new(),
                password: Vec::new(),
                password_available: false,
                method: MailAuthMethod::None,
                salt: Vec::new(),
                smtp_helo: helo.clone(),
                smtp_from: from.clone(),
                smtp_to: to.clone(),
                imap_tag: Vec::new(),
                backend: None,
            };
            match auth_http(server, context, &mut auth).await? {
                AuthReply::Accepted(_) => return Ok(AuthOutcome::Authenticated(auth)),
                AuthReply::Rejected(response) => {
                    write_client(reader, &smtp_auth_error(&response), server.timeout).await?;
                    continue;
                }
            }
        }
        invalid_smtp(reader, server, &mut errors).await?;
    }
}

async fn smtp_authenticate_method(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
    method: MailAuthMethod,
    argument: &[u8],
    helo: Vec<u8>,
    from: Vec<u8>,
    to: Vec<u8>,
) -> io::Result<Option<AuthContext>> {
    let credentials = match method {
        MailAuthMethod::Plain => {
            let mut initial = argument.to_vec();
            if initial.is_empty() {
                write_client(reader, b"334 \r\n", server.timeout).await?;
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                initial = value;
            }
            match parse_plain(&initial) {
                Some(credentials) => credentials,
                None => {
                    invalid_smtp_no_counter(reader, server).await?;
                    return Ok(None);
                }
            }
        }
        MailAuthMethod::Login => {
            let mut user =
                auth_argument_or_challenge(server, reader, argument, b"334 VXNlcm5hbWU6\r\n")
                    .await?
                    .unwrap_or_default();
            if user.is_empty() {
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                user = decode_base64(&value).unwrap_or_default();
            }
            write_client(reader, b"334 UGFzc3dvcmQ6\r\n", server.timeout).await?;
            let Some(value) =
                read_auth_response(reader, server.client_buffer_size, server.timeout).await?
            else {
                return Ok(None);
            };
            let Some(password) = decode_base64(&value) else {
                invalid_smtp_no_counter(reader, server).await?;
                return Ok(None);
            };
            Credentials { user, password }
        }
        MailAuthMethod::CramMd5 => {
            let salt = format!("<{}>\r\n", required_salt(server, context));
            let challenge =
                base64::engine::general_purpose::STANDARD.encode(trim_line(salt.as_bytes()));
            let response = format!("334 {challenge}\r\n");
            write_client(reader, response.as_bytes(), server.timeout).await?;
            let Some(value) =
                read_auth_response(reader, server.client_buffer_size, server.timeout).await?
            else {
                return Ok(None);
            };
            let Some(decoded) = decode_base64(&value) else {
                invalid_smtp_no_counter(reader, server).await?;
                return Ok(None);
            };
            let Some((user, password)) = split_once_byte(&decoded, b' ') else {
                invalid_smtp_no_counter(reader, server).await?;
                return Ok(None);
            };
            if password.len() != 32 {
                invalid_smtp_no_counter(reader, server).await?;
                return Ok(None);
            }
            return smtp_authenticate_http(
                reader,
                server,
                context,
                AuthContext {
                    user: user.to_vec(),
                    password: password.to_vec(),
                    password_available: true,
                    method,
                    salt: salt.into_bytes(),
                    smtp_helo: helo,
                    smtp_from: from,
                    smtp_to: to,
                    imap_tag: Vec::new(),
                    backend: None,
                },
            )
            .await;
        }
        MailAuthMethod::External => {
            let mut user =
                auth_argument_or_challenge(server, reader, argument, b"334 VXNlcm5hbWU6\r\n")
                    .await?
                    .unwrap_or_default();
            if user.is_empty() {
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                user = decode_base64(&value).unwrap_or_default();
            }
            Credentials {
                user,
                password: Vec::new(),
            }
        }
        MailAuthMethod::Apop | MailAuthMethod::None => return Ok(None),
    };
    smtp_authenticate_http(
        reader,
        server,
        context,
        AuthContext {
            user: credentials.user,
            password: credentials.password,
            password_available: method != MailAuthMethod::External,
            method,
            salt: Vec::new(),
            smtp_helo: helo,
            smtp_from: from,
            smtp_to: to,
            imap_tag: Vec::new(),
            backend: None,
        },
    )
    .await
}

async fn smtp_authenticate_http(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
    auth: AuthContext,
) -> io::Result<Option<AuthContext>> {
    let mut auth = auth;
    match auth_http(server, context, &mut auth).await? {
        AuthReply::Accepted(response) => {
            if let Some(user) = response.user {
                auth.user = user;
            }
            if let Some(password) = response.password {
                auth.password = password;
                auth.password_available = true;
            }
            Ok(Some(auth))
        }
        AuthReply::Rejected(response) => {
            write_client(reader, &smtp_auth_error(&response), server.timeout).await?;
            Ok(None)
        }
    }
}

async fn authenticate_pop3(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
) -> io::Result<AuthOutcome> {
    let mut errors = 0_usize;
    let mut user = Vec::new();
    let salt = required_salt(server, context);
    loop {
        let line = read_line(reader, server.client_buffer_size, server.timeout).await?;
        let Some(line) = line else {
            return Ok(AuthOutcome::Closed);
        };
        let line = trim_line(&line);
        if line.is_empty() {
            continue;
        }
        let (command, rest) = split_command(line);
        if command.eq_ignore_ascii_case(b"QUIT") {
            write_client(reader, b"+OK\r\n", server.timeout).await?;
            return Ok(AuthOutcome::Closed);
        }
        if command.eq_ignore_ascii_case(b"NOOP") {
            write_client(reader, b"+OK\r\n", server.timeout).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"CAPA") {
            let response = pop3_capa_response(server, context);
            write_client(reader, &response, server.timeout).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"STLS") {
            if context.tls.is_none() && server.tls.starttls != MailStartTls::Off {
                write_client(reader, b"+OK Begin TLS negotiation\r\n", server.timeout).await?;
                return Ok(AuthOutcome::StartTls);
            }
            invalid_pop3(reader, server, &mut errors).await?;
            continue;
        }
        if server.tls.starttls == MailStartTls::Only && context.tls.is_none() {
            invalid_pop3(reader, server, &mut errors).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"USER") {
            if rest.is_empty() {
                invalid_pop3(reader, server, &mut errors).await?;
                continue;
            }
            user = rest.to_vec();
            write_client(reader, b"+OK\r\n", server.timeout).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"PASS") {
            if user.is_empty() || rest.is_empty() {
                invalid_pop3(reader, server, &mut errors).await?;
                continue;
            }
            let mut auth = AuthContext {
                user: user.clone(),
                password: rest.to_vec(),
                password_available: true,
                method: MailAuthMethod::Plain,
                salt: Vec::new(),
                smtp_helo: Vec::new(),
                smtp_from: Vec::new(),
                smtp_to: Vec::new(),
                imap_tag: Vec::new(),
                backend: None,
            };
            match auth_http(server, context, &mut auth).await? {
                AuthReply::Accepted(response) => {
                    if let Some(password) = response.password {
                        auth.password = password;
                        auth.password_available = true;
                    }
                    return Ok(AuthOutcome::Authenticated(auth));
                }
                AuthReply::Rejected(response) => {
                    write_client(reader, &pop3_error(&response), server.timeout).await?;
                    continue;
                }
            }
        }
        if command.eq_ignore_ascii_case(b"APOP") {
            if !server.has_auth_method(MailAuthMethod::Apop) {
                invalid_pop3(reader, server, &mut errors).await?;
                continue;
            }
            let Some((username, digest)) = split_pop3_pair(rest) else {
                invalid_pop3(reader, server, &mut errors).await?;
                continue;
            };
            let mut auth = AuthContext {
                user: username.to_vec(),
                password: digest.to_vec(),
                password_available: true,
                method: MailAuthMethod::Apop,
                salt: format!("<{salt}>\r\n").into_bytes(),
                smtp_helo: Vec::new(),
                smtp_from: Vec::new(),
                smtp_to: Vec::new(),
                imap_tag: Vec::new(),
                backend: None,
            };
            match auth_http(server, context, &mut auth).await? {
                AuthReply::Accepted(response) => {
                    if let Some(password) = response.password {
                        auth.password = password;
                        auth.password_available = true;
                    }
                    return Ok(AuthOutcome::Authenticated(auth));
                }
                AuthReply::Rejected(response) => {
                    write_client(reader, &pop3_error(&response), server.timeout).await?;
                    continue;
                }
            }
        }
        if command.eq_ignore_ascii_case(b"AUTH") {
            let (method, argument) = split_sasl_method(rest);
            if method.is_empty() {
                let response = pop3_auth_methods(server);
                write_client(reader, &response, server.timeout).await?;
                continue;
            }
            let Some(method) = parse_auth_method(method) else {
                invalid_pop3(reader, server, &mut errors).await?;
                continue;
            };
            if !authentication_method_enabled(server, method) {
                invalid_pop3(reader, server, &mut errors).await?;
                continue;
            }
            if let Some(mut auth) =
                pop3_auth_method(reader, server, context, method, argument, &salt).await?
            {
                match auth_http(server, context, &mut auth).await? {
                    AuthReply::Accepted(_) => return Ok(AuthOutcome::Authenticated(auth)),
                    AuthReply::Rejected(response) => {
                        write_client(reader, &pop3_error(&response), server.timeout).await?;
                    }
                }
            }
            continue;
        }
        invalid_pop3(reader, server, &mut errors).await?;
    }
}

async fn pop3_auth_method(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
    method: MailAuthMethod,
    argument: &[u8],
    salt: &str,
) -> io::Result<Option<AuthContext>> {
    let credentials = match method {
        MailAuthMethod::Plain => {
            let mut initial = argument.to_vec();
            if initial.is_empty() {
                write_client(reader, b"+ \r\n", server.timeout).await?;
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                initial = value;
            }
            let Some(credentials) = parse_plain(&initial) else {
                invalid_pop3_no_counter(reader, server).await?;
                return Ok(None);
            };
            credentials
        }
        MailAuthMethod::Login => {
            let mut user = decode_base64(argument).unwrap_or_default();
            if user.is_empty() {
                write_client(reader, b"+ VXNlcm5hbWU6\r\n", server.timeout).await?;
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                user = decode_base64(&value).unwrap_or_default();
            }
            write_client(reader, b"+ UGFzc3dvcmQ6\r\n", server.timeout).await?;
            let Some(value) =
                read_auth_response(reader, server.client_buffer_size, server.timeout).await?
            else {
                return Ok(None);
            };
            let Some(password) = decode_base64(&value) else {
                invalid_pop3_no_counter(reader, server).await?;
                return Ok(None);
            };
            Credentials { user, password }
        }
        MailAuthMethod::CramMd5 => {
            let salt = format!("<{salt}>\r\n");
            let challenge =
                base64::engine::general_purpose::STANDARD.encode(trim_line(salt.as_bytes()));
            let response = format!("+ {challenge}\r\n");
            write_client(reader, response.as_bytes(), server.timeout).await?;
            let Some(value) =
                read_auth_response(reader, server.client_buffer_size, server.timeout).await?
            else {
                return Ok(None);
            };
            let Some(decoded) = decode_base64(&value) else {
                invalid_pop3_no_counter(reader, server).await?;
                return Ok(None);
            };
            let Some((user, password)) = split_once_byte(&decoded, b' ') else {
                invalid_pop3_no_counter(reader, server).await?;
                return Ok(None);
            };
            if password.len() != 32 {
                invalid_pop3_no_counter(reader, server).await?;
                return Ok(None);
            }
            return Ok(Some(AuthContext {
                user: user.to_vec(),
                password: password.to_vec(),
                password_available: true,
                method,
                salt: salt.into_bytes(),
                smtp_helo: Vec::new(),
                smtp_from: Vec::new(),
                smtp_to: Vec::new(),
                imap_tag: Vec::new(),
                backend: None,
            }));
        }
        MailAuthMethod::External => {
            let mut user = decode_base64(argument).unwrap_or_default();
            if user.is_empty() {
                write_client(reader, b"+ VXNlcm5hbWU6\r\n", server.timeout).await?;
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                user = decode_base64(&value).unwrap_or_default();
            }
            Credentials {
                user,
                password: Vec::new(),
            }
        }
        MailAuthMethod::Apop | MailAuthMethod::None => return Ok(None),
    };
    let _ = context;
    Ok(Some(AuthContext {
        user: credentials.user,
        password: credentials.password,
        password_available: method != MailAuthMethod::External,
        method,
        salt: if method == MailAuthMethod::CramMd5 {
            format!("<{salt}>").into_bytes()
        } else {
            Vec::new()
        },
        smtp_helo: Vec::new(),
        smtp_from: Vec::new(),
        smtp_to: Vec::new(),
        imap_tag: Vec::new(),
        backend: None,
    }))
}

async fn authenticate_imap(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
) -> io::Result<AuthOutcome> {
    let mut errors = 0_usize;
    loop {
        let line = read_line(reader, server.client_buffer_size, server.timeout).await?;
        let Some(line) = line else {
            return Ok(AuthOutcome::Closed);
        };
        let line = trim_line(&line);
        if line.is_empty() {
            continue;
        }
        let Some(tag_end) = line.iter().position(u8::is_ascii_whitespace) else {
            invalid_imap(reader, server, line, &mut errors).await?;
            continue;
        };
        let tag = &line[..tag_end];
        if tag.is_empty() || tag.len() > 32 {
            invalid_imap(reader, server, line, &mut errors).await?;
            continue;
        }
        let command_line = trim_start_ascii(&line[tag_end..]);
        let (command, rest) = split_command(command_line);
        if command.eq_ignore_ascii_case(b"NOOP") {
            write_imap_ok(reader, server, tag, b"completed").await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"LOGOUT") {
            write_client(reader, b"* BYE\r\n", server.timeout).await?;
            write_imap_ok(reader, server, tag, b"completed").await?;
            return Ok(AuthOutcome::Closed);
        }
        if command.eq_ignore_ascii_case(b"CAPABILITY") {
            let response = imap_capability_response(server, context);
            write_client(reader, &response, server.timeout).await?;
            write_imap_ok(reader, server, tag, b"completed").await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"STARTTLS") {
            if context.tls.is_none() && server.tls.starttls != MailStartTls::Off {
                write_imap_ok(reader, server, tag, b"Begin TLS negotiation now").await?;
                return Ok(AuthOutcome::StartTls);
            }
            invalid_imap(reader, server, line, &mut errors).await?;
            continue;
        }
        if server.tls.starttls == MailStartTls::Only && context.tls.is_none() {
            invalid_imap(reader, server, line, &mut errors).await?;
            continue;
        }
        if command.eq_ignore_ascii_case(b"LOGIN") {
            let Some(arguments) =
                parse_imap_login_arguments(reader, rest, server.client_buffer_size, server.timeout)
                    .await?
            else {
                invalid_imap(reader, server, line, &mut errors).await?;
                continue;
            };
            if arguments.len() != 2 || arguments[0].is_empty() {
                invalid_imap(reader, server, line, &mut errors).await?;
                continue;
            }
            let auth = AuthContext {
                user: arguments[0].clone(),
                password: arguments[1].clone(),
                password_available: true,
                method: MailAuthMethod::Plain,
                salt: Vec::new(),
                smtp_helo: Vec::new(),
                smtp_from: Vec::new(),
                smtp_to: Vec::new(),
                imap_tag: tag.to_vec(),
                backend: None,
            };
            let mut auth = auth;
            match auth_http(server, context, &mut auth).await? {
                AuthReply::Accepted(response) => {
                    if let Some(password) = response.password {
                        auth.password = password;
                        auth.password_available = true;
                    }
                    return Ok(AuthOutcome::Authenticated(auth));
                }
                AuthReply::Rejected(response) => {
                    write_client(reader, &imap_error(tag, &response), server.timeout).await?;
                    continue;
                }
            }
        }
        if command.eq_ignore_ascii_case(b"AUTHENTICATE") {
            let (method, argument) = split_sasl_method(rest);
            if method.is_empty() {
                invalid_imap(reader, server, line, &mut errors).await?;
                continue;
            }
            let Some(method_id) = parse_auth_method(method) else {
                invalid_imap(reader, server, line, &mut errors).await?;
                continue;
            };
            if !authentication_method_enabled(server, method_id) {
                write_imap_no(reader, server, tag, b"unsupported authentication").await?;
                continue;
            }
            let Some(mut auth) =
                imap_auth_method(reader, server, context, tag, method_id, argument).await?
            else {
                continue;
            };
            match auth_http(server, context, &mut auth).await? {
                AuthReply::Accepted(response) => {
                    if let Some(password) = response.password {
                        auth.password = password;
                    }
                    return Ok(AuthOutcome::Authenticated(auth));
                }
                AuthReply::Rejected(response) => {
                    write_client(reader, &imap_error(tag, &response), server.timeout).await?;
                    continue;
                }
            }
        }
        invalid_imap(reader, server, line, &mut errors).await?;
        if has_imap_non_sync_literal(line) {
            return Ok(AuthOutcome::Closed);
        }
    }
}

async fn imap_auth_method(
    reader: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
    tag: &[u8],
    method: MailAuthMethod,
    argument: &[u8],
) -> io::Result<Option<AuthContext>> {
    let credentials = match method {
        MailAuthMethod::Plain => {
            let mut initial = argument.to_vec();
            if initial.is_empty() {
                write_client(reader, b"+ \r\n", server.timeout).await?;
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                initial = value;
            }
            let Some(credentials) = parse_plain(&initial) else {
                write_imap_no(reader, server, tag, b"invalid command").await?;
                return Ok(None);
            };
            credentials
        }
        MailAuthMethod::Login => {
            let mut user = decode_base64(argument).unwrap_or_default();
            if user.is_empty() {
                write_client(reader, b"+ VXNlcm5hbWU6\r\n", server.timeout).await?;
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                user = decode_base64(&value).unwrap_or_default();
            }
            write_client(reader, b"+ UGFzc3dvcmQ6\r\n", server.timeout).await?;
            let Some(value) =
                read_auth_response(reader, server.client_buffer_size, server.timeout).await?
            else {
                return Ok(None);
            };
            let Some(password) = decode_base64(&value) else {
                write_imap_no(reader, server, tag, b"invalid command").await?;
                return Ok(None);
            };
            Credentials { user, password }
        }
        MailAuthMethod::CramMd5 => {
            let salt = format!("<{}>\r\n", required_salt(server, context));
            let challenge =
                base64::engine::general_purpose::STANDARD.encode(trim_line(salt.as_bytes()));
            let response = format!("+ {challenge}\r\n");
            write_client(reader, response.as_bytes(), server.timeout).await?;
            let Some(value) =
                read_auth_response(reader, server.client_buffer_size, server.timeout).await?
            else {
                return Ok(None);
            };
            let Some(decoded) = decode_base64(&value) else {
                write_imap_no(reader, server, tag, b"invalid command").await?;
                return Ok(None);
            };
            let Some((user, password)) = split_once_byte(&decoded, b' ') else {
                write_imap_no(reader, server, tag, b"invalid command").await?;
                return Ok(None);
            };
            if password.len() != 32 {
                write_imap_no(reader, server, tag, b"invalid command").await?;
                return Ok(None);
            }
            return Ok(Some(AuthContext {
                user: user.to_vec(),
                password: password.to_vec(),
                password_available: true,
                method,
                salt: salt.into_bytes(),
                smtp_helo: Vec::new(),
                smtp_from: Vec::new(),
                smtp_to: Vec::new(),
                imap_tag: tag.to_vec(),
                backend: None,
            }));
        }
        MailAuthMethod::External => {
            let mut user = decode_base64(argument).unwrap_or_default();
            if user.is_empty() {
                write_client(reader, b"+ VXNlcm5hbWU6\r\n", server.timeout).await?;
                let Some(value) =
                    read_auth_response(reader, server.client_buffer_size, server.timeout).await?
                else {
                    return Ok(None);
                };
                user = decode_base64(&value).unwrap_or_default();
            }
            Credentials {
                user,
                password: Vec::new(),
            }
        }
        MailAuthMethod::Apop | MailAuthMethod::None => return Ok(None),
    };
    Ok(Some(AuthContext {
        user: credentials.user,
        password: credentials.password,
        password_available: method != MailAuthMethod::External,
        method,
        salt: Vec::new(),
        smtp_helo: Vec::new(),
        smtp_from: Vec::new(),
        smtp_to: Vec::new(),
        imap_tag: tag.to_vec(),
        backend: None,
    }))
}

async fn parse_imap_login_arguments(
    reader: &mut MailReader,
    first: &[u8],
    max_line: usize,
    timeout: Duration,
) -> io::Result<Option<Vec<Vec<u8>>>> {
    let mut source = first.to_vec();
    let mut arguments = Vec::new();
    loop {
        let position = trim_start_position(&source);
        if position == source.len() {
            return Ok(Some(arguments));
        }
        if source[position] == b'"' {
            let mut cursor = position + 1;
            let mut value = Vec::new();
            let mut closed = false;
            while cursor < source.len() {
                match source[cursor] {
                    b'\\' if cursor + 1 < source.len() => {
                        value.push(source[cursor + 1]);
                        cursor += 2;
                    }
                    b'"' => {
                        cursor += 1;
                        closed = true;
                        break;
                    }
                    byte => {
                        value.push(byte);
                        cursor += 1;
                    }
                }
            }
            if !closed {
                return Ok(None);
            }
            arguments.push(value);
            source.drain(..cursor);
            continue;
        }
        if source[position] == b'{' {
            let close = source[position..]
                .iter()
                .position(|byte| *byte == b'}')
                .map(|offset| position + offset);
            let Some(close) = close else {
                return Ok(None);
            };
            if close + 1 != source.len() {
                return Ok(None);
            }
            let literal = &source[position + 1..close];
            let non_sync = literal.ends_with(b"+");
            let literal = literal.strip_suffix(b"+").unwrap_or(literal);
            let Some(length) = parse_imap_literal_length(literal) else {
                return Ok(None);
            };
            if length > max_line || source.len() > max_line {
                return Ok(None);
            }
            if !non_sync {
                write_client(reader, b"+ \r\n", timeout).await?;
            }
            let mut value = vec![0_u8; length];
            match tokio::time::timeout(timeout, reader.read_exact(&mut value)).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "IMAP literal timed out",
                    ));
                }
            }
            arguments.push(value);
            let Some(line) = read_line(reader, max_line, timeout).await? else {
                return Ok(None);
            };
            source = trim_line(&line).to_vec();
            continue;
        }
        let token_start = position;
        let mut position = position;
        while position < source.len() && !source[position].is_ascii_whitespace() {
            position += 1;
        }
        arguments.push(source[token_start..position].to_vec());
        source.drain(..position);
    }
}

fn parse_imap_literal_length(value: &[u8]) -> Option<usize> {
    if value.is_empty() || value.len() > 20 {
        return None;
    }
    let mut length = 0_usize;
    for byte in value {
        if !byte.is_ascii_digit() {
            return None;
        }
        length = length
            .checked_mul(10)?
            .checked_add(usize::from(byte - b'0'))?;
    }
    Some(length)
}

async fn auth_http(
    server: &MailServer,
    context: &SessionContext,
    auth: &mut AuthContext,
) -> io::Result<AuthReply> {
    let client_ip = context
        .remote
        .socket_addr()
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|| "unix".into());
    let mut login_attempt = 1_usize;
    loop {
        let request = AuthHttpRequest {
            method: auth.method.auth_http_name(),
            user: &auth.user,
            password: &auth.password,
            salt: &auth.salt,
            protocol: server.protocol,
            login_attempt,
            client_ip: &client_ip,
            client_host: context.client_host.as_deref(),
            proxy_addresses: Some(&context.proxy_addresses),
            smtp_helo: &auth.smtp_helo,
            smtp_from: &auth.smtp_from,
            smtp_to: &auth.smtp_to,
            tls: context.tls.as_ref(),
            client_certificate_verify: context.client_certificate_verify.as_deref(),
        };
        let response = query_auth_http(&server.auth_http, &request).await?;
        match response.status {
            AuthHttpStatus::Ok => {
                if let Some(user) = &response.user {
                    auth.user.clone_from(user);
                }
                if let Some(password) = &response.password {
                    auth.password.clone_from(password);
                    auth.password_available = true;
                }
                let Some(server_name) = response.server.clone() else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "auth_http did not send Auth-Server",
                    ));
                };
                let Some(port) = response.port else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "auth_http did not send Auth-Port",
                    ));
                };
                auth.backend = Some((server_name, port));
                return Ok(AuthReply::Accepted(response));
            }
            AuthHttpStatus::Wait => {
                if !response.wait.is_zero() {
                    tokio::time::sleep(response.wait).await;
                }
                login_attempt = login_attempt.saturating_add(1);
            }
            AuthHttpStatus::Failed { .. } => return Ok(AuthReply::Rejected(response)),
        }
    }
}

fn smtp_helo_response(server: &MailServer, context: &SessionContext, esmtp: bool) -> Vec<u8> {
    if !esmtp {
        return format!("250 {}\r\n", server.server_name).into_bytes();
    }
    let mut lines: Vec<String> = server.capabilities.clone();
    if server.tls.starttls != MailStartTls::Only || context.tls.is_some() {
        let methods: Vec<_> = server
            .auth_methods
            .iter()
            .filter(|method| **method != MailAuthMethod::None)
            .map(|method| match method {
                MailAuthMethod::Plain => "PLAIN",
                MailAuthMethod::Login => "LOGIN",
                MailAuthMethod::CramMd5 => "CRAM-MD5",
                MailAuthMethod::External => "EXTERNAL",
                MailAuthMethod::Apop | MailAuthMethod::None => "",
            })
            .filter(|method| !method.is_empty())
            .collect();
        if !methods.is_empty() {
            lines.push(format!("AUTH {}", methods.join(" ")));
        }
    }
    if server.tls.starttls != MailStartTls::Off && context.tls.is_none() {
        lines.push("STARTTLS".into());
    }
    if lines.is_empty() {
        return format!("250 {}\r\n", server.server_name).into_bytes();
    }
    let mut response = format!("250-{}\r\n", server.server_name);
    for (index, line) in lines.iter().enumerate() {
        if index + 1 == lines.len() {
            response.push_str("250 ");
        } else {
            response.push_str("250-");
        }
        response.push_str(line);
        response.push_str("\r\n");
    }
    response.into_bytes()
}

fn pop3_capa_response(server: &MailServer, context: &SessionContext) -> Vec<u8> {
    let mut response = b"+OK Capability list follows\r\n".to_vec();
    for capability in &server.capabilities {
        if server.tls.starttls == MailStartTls::Only
            && context.tls.is_none()
            && capability.eq_ignore_ascii_case("USER")
        {
            continue;
        }
        response.extend_from_slice(capability.as_bytes());
        response.extend_from_slice(b"\r\n");
    }
    if server.tls.starttls != MailStartTls::Off && context.tls.is_none() {
        response.extend_from_slice(b"STLS\r\n");
    }
    if server.tls.starttls != MailStartTls::Only || context.tls.is_some() {
        let methods: Vec<_> = server
            .auth_methods
            .iter()
            .filter_map(|method| match method {
                MailAuthMethod::Plain => Some("PLAIN"),
                MailAuthMethod::Login => Some("LOGIN"),
                MailAuthMethod::CramMd5 => Some("CRAM-MD5"),
                MailAuthMethod::External => Some("EXTERNAL"),
                MailAuthMethod::Apop | MailAuthMethod::None => None,
            })
            .collect();
        response.extend_from_slice(b"SASL");
        for method in methods {
            response.push(b' ');
            response.extend_from_slice(method.as_bytes());
        }
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b".\r\n");
    response
}

fn pop3_auth_methods(server: &MailServer) -> Vec<u8> {
    let mut response = b"+OK methods supported:\r\n".to_vec();
    for method in &server.auth_methods {
        let name = match method {
            MailAuthMethod::Plain => "PLAIN",
            MailAuthMethod::Login => "LOGIN",
            MailAuthMethod::CramMd5 => "CRAM-MD5",
            MailAuthMethod::External => "EXTERNAL",
            MailAuthMethod::Apop | MailAuthMethod::None => continue,
        };
        response.extend_from_slice(name.as_bytes());
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b".\r\n");
    response
}

fn imap_capability_response(server: &MailServer, context: &SessionContext) -> Vec<u8> {
    let mut response = b"* CAPABILITY".to_vec();
    for capability in &server.capabilities {
        response.push(b' ');
        response.extend_from_slice(capability.as_bytes());
    }
    if server.tls.starttls != MailStartTls::Only || context.tls.is_some() {
        for method in &server.auth_methods {
            let name = match method {
                MailAuthMethod::Plain => "AUTH=PLAIN",
                MailAuthMethod::Login => "AUTH=LOGIN",
                MailAuthMethod::CramMd5 => "AUTH=CRAM-MD5",
                MailAuthMethod::External => "AUTH=EXTERNAL",
                MailAuthMethod::Apop | MailAuthMethod::None => continue,
            };
            response.push(b' ');
            response.extend_from_slice(name.as_bytes());
        }
    }
    if server.tls.starttls != MailStartTls::Off && context.tls.is_none() {
        response.extend_from_slice(b" STARTTLS");
    }
    if server.tls.starttls == MailStartTls::Only && context.tls.is_none() {
        response.extend_from_slice(b" LOGINDISABLED");
    }
    response.extend_from_slice(b"\r\n");
    response
}

async fn write_imap_ok(
    reader: &mut MailReader,
    server: &MailServer,
    tag: &[u8],
    message: &[u8],
) -> io::Result<()> {
    let mut response = tag.to_vec();
    response.extend_from_slice(b" OK ");
    response.extend_from_slice(message);
    response.extend_from_slice(b"\r\n");
    write_client(reader, &response, server.timeout).await
}

async fn write_imap_no(
    reader: &mut MailReader,
    server: &MailServer,
    tag: &[u8],
    message: &[u8],
) -> io::Result<()> {
    let mut response = tag.to_vec();
    response.extend_from_slice(b" NO ");
    response.extend_from_slice(message);
    response.extend_from_slice(b"\r\n");
    write_client(reader, &response, server.timeout).await
}

async fn invalid_smtp(
    reader: &mut MailReader,
    server: &MailServer,
    errors: &mut usize,
) -> io::Result<()> {
    *errors += 1;
    if *errors >= server.max_errors {
        write_client(reader, b"500 5.5.1 Invalid command\r\n", server.timeout).await?;
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "too many invalid SMTP commands",
        ));
    }
    write_client(reader, b"500 5.5.1 Invalid command\r\n", server.timeout).await
}

async fn invalid_smtp_no_counter(reader: &mut MailReader, server: &MailServer) -> io::Result<()> {
    write_client(reader, b"501 5.5.4 Invalid argument\r\n", server.timeout).await
}

async fn invalid_pop3(
    reader: &mut MailReader,
    server: &MailServer,
    errors: &mut usize,
) -> io::Result<()> {
    *errors += 1;
    if *errors >= server.max_errors {
        write_client(reader, b"-ERR invalid command\r\n", server.timeout).await?;
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "too many invalid POP3 commands",
        ));
    }
    write_client(reader, b"-ERR invalid command\r\n", server.timeout).await
}

async fn invalid_pop3_no_counter(reader: &mut MailReader, server: &MailServer) -> io::Result<()> {
    write_client(reader, b"-ERR invalid command\r\n", server.timeout).await
}

async fn invalid_imap(
    reader: &mut MailReader,
    server: &MailServer,
    line: &[u8],
    errors: &mut usize,
) -> io::Result<()> {
    *errors += 1;
    let tag = line
        .iter()
        .position(u8::is_ascii_whitespace)
        .map(|position| &line[..position])
        .filter(|tag| !tag.is_empty())
        .unwrap_or(b"*");
    let mut response = tag.to_vec();
    response.extend_from_slice(b" BAD invalid command\r\n");
    if *errors >= server.max_errors {
        write_client(reader, &response, server.timeout).await?;
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "too many invalid IMAP commands",
        ));
    }
    write_client(reader, &response, server.timeout).await
}

fn smtp_auth_error(response: &AuthHttpResponse) -> Vec<u8> {
    let AuthHttpStatus::Failed { code, message } = &response.status else {
        return b"535 5.7.1 Authentication failed\r\n".to_vec();
    };
    let code = code.as_deref().unwrap_or("535 5.7.1");
    format!("{code} {message}\r\n").into_bytes()
}

fn pop3_error(response: &AuthHttpResponse) -> Vec<u8> {
    let message = failed_message(response);
    format!("-ERR {message}\r\n").into_bytes()
}

fn imap_error(tag: &[u8], response: &AuthHttpResponse) -> Vec<u8> {
    let message = failed_message(response);
    let mut response = tag.to_vec();
    response.extend_from_slice(b" NO ");
    response.extend_from_slice(message.as_bytes());
    response.extend_from_slice(b"\r\n");
    response
}

fn failed_message(response: &AuthHttpResponse) -> &str {
    match &response.status {
        AuthHttpStatus::Failed { message, .. } => message,
        _ => "authentication failed",
    }
}

struct Credentials {
    user: Vec<u8>,
    password: Vec<u8>,
}

fn parse_plain(value: &[u8]) -> Option<Credentials> {
    let decoded = decode_base64(value)?;
    let first = decoded.iter().position(|byte| *byte == 0)?;
    let user_start = first + 1;
    let second = decoded[user_start..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|offset| user_start + offset)?;
    Some(Credentials {
        user: decoded[user_start..second].to_vec(),
        password: decoded[second + 1..].to_vec(),
    })
}

fn decode_base64(value: &[u8]) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(trim_ascii(value))
        .ok()
}

async fn auth_argument_or_challenge(
    server: &MailServer,
    reader: &mut MailReader,
    argument: &[u8],
    challenge: &[u8],
) -> io::Result<Option<Vec<u8>>> {
    let argument = trim_start_ascii(argument);
    if !argument.is_empty() {
        return Ok(Some(
            decode_base64(argument).unwrap_or_else(|| argument.to_vec()),
        ));
    }
    write_client(reader, challenge, server.timeout).await?;
    let Some(value) = read_auth_response(reader, server.client_buffer_size, server.timeout).await?
    else {
        return Ok(None);
    };
    Ok(Some(decode_base64(&value).unwrap_or(value)))
}

async fn read_auth_response(
    reader: &mut MailReader,
    max_line: usize,
    timeout: Duration,
) -> io::Result<Option<Vec<u8>>> {
    let line = read_line(reader, max_line, timeout).await?;
    Ok(line.map(|line| trim_line(&line).to_vec()))
}

fn split_sasl_method(rest: &[u8]) -> (&[u8], &[u8]) {
    let rest = trim_start_ascii(rest);
    let end = rest
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(rest.len());
    (&rest[..end], trim_start_ascii(&rest[end..]))
}

fn parse_auth_method(value: &[u8]) -> Option<MailAuthMethod> {
    if value.eq_ignore_ascii_case(b"PLAIN") {
        Some(MailAuthMethod::Plain)
    } else if value.eq_ignore_ascii_case(b"LOGIN") {
        Some(MailAuthMethod::Login)
    } else if value.eq_ignore_ascii_case(b"CRAM-MD5") {
        Some(MailAuthMethod::CramMd5)
    } else if value.eq_ignore_ascii_case(b"EXTERNAL") {
        Some(MailAuthMethod::External)
    } else {
        None
    }
}

fn authentication_method_enabled(server: &MailServer, method: MailAuthMethod) -> bool {
    matches!(method, MailAuthMethod::Plain | MailAuthMethod::Login)
        || server.has_auth_method(method)
}

fn has_imap_non_sync_literal(line: &[u8]) -> bool {
    let Some(close) = line.iter().rposition(|byte| *byte == b'}') else {
        return false;
    };
    if close + 1 != line.len() || close == 0 || line[close - 1] != b'+' {
        return false;
    }
    let mut position = close - 1;
    let digits_end = position;
    while position > 0 && line[position - 1].is_ascii_digit() {
        position -= 1;
    }
    position != digits_end && position > 0 && line[position - 1] == b'{'
}

fn split_pop3_pair(value: &[u8]) -> Option<(&[u8], &[u8])> {
    let value = trim_start_ascii(value);
    let position = value.iter().position(u8::is_ascii_whitespace)?;
    let first = &value[..position];
    let second = trim_start_ascii(&value[position..]);
    if first.is_empty() || second.is_empty() {
        None
    } else {
        Some((first, second))
    }
}

fn split_command(line: &[u8]) -> (&[u8], &[u8]) {
    let position = line
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(line.len());
    (&line[..position], trim_start_ascii(&line[position..]))
}

fn trim_line(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn trim_start_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    value
}

fn trim_start_position(value: &[u8]) -> usize {
    value
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(value.len())
}

fn split_once_byte(value: &[u8], delimiter: u8) -> Option<(&[u8], &[u8])> {
    let position = value.iter().position(|byte| *byte == delimiter)?;
    Some((&value[..position], &value[position + 1..]))
}

pub(super) async fn write_client(
    reader: &mut MailReader,
    data: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    match tokio::time::timeout(timeout, async {
        let stream = reader.get_mut();
        stream.write_all(data).await?;
        stream.flush().await
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "mail client write timed out",
        )),
    }
}

pub(super) async fn read_line(
    reader: &mut MailReader,
    max_len: usize,
    timeout: Duration,
) -> io::Result<Option<Vec<u8>>> {
    let operation = async {
        let mut line = Vec::new();
        loop {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "mail client closed an incomplete command",
                    ))
                };
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|position| position + 1)
                .unwrap_or(available.len());
            if line.len().saturating_add(take) > max_len.saturating_add(2) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "mail command is too long",
                ));
            }
            line.extend_from_slice(&available[..take]);
            let complete = available[take - 1] == b'\n';
            reader.consume(take);
            if complete {
                return Ok(Some(line));
            }
        }
    };
    match tokio::time::timeout(timeout, operation).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "mail client read timed out",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_credentials() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"\0user\0secret");
        let credentials = parse_plain(encoded.as_bytes()).unwrap();
        assert_eq!(credentials.user, b"user");
        assert_eq!(credentials.password, b"secret");
    }

    #[test]
    fn parses_imap_literal_lengths() {
        assert_eq!(parse_imap_literal_length(b"128"), Some(128));
        assert_eq!(parse_imap_literal_length(b""), None);
        assert_eq!(parse_imap_literal_length(b"-1"), None);
    }
}
