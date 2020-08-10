use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use actix_files::NamedFile;
use actix_multipart::Multipart;
use actix_web::{middleware, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use env_logger::Env;
use futures::{StreamExt, TryStreamExt};


static WEB_ROOT: &str = "conda/";


async fn index() -> Result<NamedFile, Error> {
    let mut filepath = PathBuf::from(WEB_ROOT);
    filepath.push("index.html");
    Ok(NamedFile::open(filepath)?)
}

async fn channel_index(info: web::Path<(String,)>) -> Result<NamedFile, Error> {
    let channel = &info.0;
    let mut filepath = PathBuf::from(WEB_ROOT);
    filepath.push(channel);
    filepath.push("index.html");
    Ok(NamedFile::open(filepath)?)
}

async fn channel(req: HttpRequest) -> Result<NamedFile, Error> {
    let channel: PathBuf = req.match_info().query("channel").parse().unwrap();
    let path: PathBuf = req.match_info().query("filename").parse().unwrap();
    let mut filepath = PathBuf::from(WEB_ROOT);
    filepath.push(channel);
    filepath.push(path);
    if filepath.is_dir() {
        filepath.push("index.html");
        Ok(NamedFile::open(filepath)?)
    } else {
        Ok(NamedFile::open(filepath)?)
    }
}

async fn upload(info: web::Path<(String,)>, mut payload: Multipart) -> Result<HttpResponse, Error> {
    let channel = &info.0;
    let dirpath = format!("{}/{}", WEB_ROOT, channel);
    while let Ok(Some(mut field)) = payload.try_next().await {
        let content_disposition = field.content_disposition().unwrap();
        let mut filepath = PathBuf::from(&dirpath);

        // if already exists, return 409
        if filepath.exists() {
            return Ok(HttpResponse::Conflict().into());
        }

        fs::create_dir_all(&dirpath)?;

        filepath.push(content_disposition.get_filename().unwrap().split("/").last().unwrap());
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
async fn main() -> std::io::Result<()> {
    env_logger::from_env(Env::default().default_filter_or("info")).init();
    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .service(
                web::resource("/")
                    .route(web::post().to(upload))
                    .route(web::get().to(index)))
            .service(
                web::resource("/{channel}/")
                    .route(web::post().to(upload))
                    .route(web::get().to(channel_index)))
            .service(
                web::resource("/{channel}/{filename:.*}")
                    .route(web::get().to(channel)))
    })
    .bind("127.0.0.1:8088")?
    .workers(1)
    .run()
    .await
}
