use super::*;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) async fn handle(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
) -> Response {
    if env.uri.ends_with('/') {
        return error::status(409);
    }
    if request.header("content-range").is_some() {
        return error::status(501);
    }

    let path = path::source_path(context, location, &env.uri);
    let existed = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_dir() => return error::status(409),
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return error::io_error(&error, 500),
    };
    if context.dav.create_full_put_path
        && let Err(error) = fs_ops::create_parent_dirs(&path, context.dav.access).await
    {
        return error::io_error(&error, 500);
    }

    let (temp, mut file) = match create_temp(&path).await {
        Ok(temp) => temp,
        Err(error) => return error::io_error(&error, 500),
    };
    let write_result = async {
        if let Some(body_stream) = request
            .body_stream
            .as_ref()
            .filter(|stream| stream.is_file_backed())
            .cloned()
        {
            while let Some(chunk) = body_stream.next().await? {
                file.write_all(&chunk).await?;
            }
        } else {
            file.write_all(&request.body).await?;
        }
        file.sync_all().await?;
        fs_ops::set_mode(&temp, context.dav.access)
    }
    .await;
    if let Err(error) = write_result {
        let _ = tokio::fs::remove_file(&temp).await;
        return error::io_error(&error, 500);
    }
    let modified = request
        .header("date")
        .and_then(|value| httpdate::parse_http_date(value).ok());
    let _ = fs_ops::set_modified(&temp, modified);
    if let Err(error) = tokio::fs::rename(&temp, &path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return error::io_error(&error, 500);
    }

    let status = if existed { 204 } else { 201 };
    let mut response = Response::empty(status);
    if !existed {
        response.set_header("Location", path::location_header(&env.uri));
    }
    response
}

async fn create_temp(path: &Path) -> io::Result<(PathBuf, tokio::fs::File)> {
    for _ in 0..32 {
        let temp = temp_path(path);
        match tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .await
        {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate a DAV temporary file",
    ))
}

fn temp_path(path: &Path) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = format!(".starling-dav-{}-{counter}", std::process::id());
    path.with_file_name(temp)
}
