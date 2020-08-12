use std::fs;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use actix_files::NamedFile;
use actix_multipart::Multipart;
use actix_web::{middleware, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use env_logger::Env;
use futures::{StreamExt, TryStreamExt};
use serde_derive::Deserialize;

#[derive(Clone, Deserialize)]
struct Config {
    bind: String,
    root: String,
}

async fn index(config: web::Data<Config>) -> Result<HttpResponse, Error> {
    let mut html = String::from(r#"
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">
                <title>Conda Hoster</title>
            </head>
            <body>
                <p>Available Channels:</p>
    "#);
    for entry in PathBuf::from(&config.root).read_dir()? {
        let _ = entry.map(|entry| {
            if entry.path().is_dir() {
                html.push_str(&format!("<p><a href=\"{0}/\">{0}</a></p>", entry.file_name().to_str().unwrap()));
            }
        });
    }
    html.push_str("</body>\n</html>\n");
    return Ok(HttpResponse::Ok().header("Content-Type", "text/html; charset=utf-8").body(html));
}

async fn channel_index(config: web::Data<Config>, info: web::Path<(String,)>) -> Result<NamedFile, Error> {
    let channel = &info.0;
    let mut filepath = PathBuf::from(&config.root);
    filepath.push(channel);
    filepath.push("index.html");
    Ok(NamedFile::open(filepath)?)
}

async fn channel(config: web::Data<Config>, req: HttpRequest) -> Result<NamedFile, Error> {
    let channel: PathBuf = req.match_info().query("channel").parse().unwrap();
    let path: PathBuf = req.match_info().query("filename").parse().unwrap();
    let mut filepath = PathBuf::from(&config.root);
    filepath.push(channel);
    filepath.push(path);
    if filepath.is_dir() {
        filepath.push("index.html");
        Ok(NamedFile::open(filepath)?)
    } else {
        Ok(NamedFile::open(filepath)?)
    }
}

async fn upload(config: web::Data<Config>, info: web::Path<(String,String)>, mut payload: Multipart) -> Result<HttpResponse, Error> {
    let channel = &info.0;
    let arch = &info.1;
    let dirpath = format!("{}/{}/{}", &config.root, channel, arch);
    while let Ok(Some(mut field)) = payload.try_next().await {
        let content_disposition = field.content_disposition().unwrap();

        let mut filepath = PathBuf::from(&dirpath);
        filepath.push(content_disposition.get_filename().unwrap().split("/").last().unwrap());

        // if already exists, return 409
        if filepath.exists() {
            return Ok(HttpResponse::Conflict().into());
        }

        fs::create_dir_all(&dirpath)?;

        let mut f = web::block(|| std::fs::File::create(filepath)).await.unwrap();
        while let Some(chunk) = field.next().await {
            let data = chunk.unwrap();
            f = web::block(move || f.write_all(&data).map(|_| f)).await?;
        }
    }

    // do indexing
    let mut p = Command::new("conda")
        .arg("index")
        .arg(&dirpath)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    let index_result = web::block(move || p.wait()).await?;

    if !index_result.success() {
        return Err(HttpResponse::InternalServerError().into());
    }

    Ok(HttpResponse::Ok().into())
}

#[actix_rt::main]
async fn main() -> io::Result<()> {
    env_logger::from_env(Env::default().default_filter_or("info")).init();

    let config_dirs = [
        dirs::config_dir().unwrap().join("conda-hoster"),
        PathBuf::from("/etc").join("conda-hoster")
    ];

    let mut config_path_opt: Option<PathBuf> = None;
    for config_dir in config_dirs.iter() {
        let config_path = config_dir.join("config.toml");
        if config_path.exists() {
            config_path_opt = Some(config_path);
            break;
        }
    }

    let config_path = config_path_opt.unwrap_or(config_dirs[0].join("config.toml"));
    if !config_path.exists() {
        fs::create_dir_all(&config_dirs[0])?;
        fs::write(&config_path, format!(
r#"bind = "0.0.0.0:8088"
root = "{}/conda-hoster/web-root"
"#, dirs::data_dir().unwrap().to_str().unwrap()))?;
    }

    let config: Config;
    let config_bytes = fs::read(&config_path)?;
    let config_string = String::from_utf8(config_bytes);
    match config_string {
        Ok(config_string) => {
            let config_obj = toml::from_str(&config_string);
            match config_obj {
                Ok(config_obj) => {
                    config = config_obj;
                },
                Err(e) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("failed to parse {}: {}", config_path.to_str().unwrap(), e)));
                }
            }
        },
        Err(e) => {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("configuration file {} is not correctly UTF-8 encoded: {}", config_path.to_str().unwrap(), e)));
        }
    }

    fs::create_dir_all(&config.root)?;

    let config_clone = config.clone();
    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .data(config_clone.clone())
            .service(
                web::resource("/")
                    .route(web::get().to(index)))
            .service(
                web::resource("/{channel}/{arch}/")
                    .route(web::post().to(upload))
                    .route(web::get().to(channel_index)))
            .service(
                web::resource("/{channel}/{filename:.*}")
                    .route(web::get().to(channel)))
    })
    .bind(&config.bind)?
    .workers(1)
    .run()
    .await
}
