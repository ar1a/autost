use std::{
    env::{self},
    fs::{create_dir_all, File},
    path::Path,
};

use jane_eyre::eyre::{self, bail, OptionExt};
use reqwest::{
    header::{self, HeaderMap, HeaderValue},
    Client,
};
use tracing::info;

use crate::cohost::{
    ListEditedProjectsResponse, LoggedInResponse, Post, PostsResponse, TrpcResponse,
};

#[derive(clap::Args, Debug)]
pub struct Cohost2json {
    pub project_name: String,
    pub path_to_chosts: String,
}

pub async fn main(args: Cohost2json) -> eyre::Result<()> {
    let requested_project = args.project_name;
    let output_path = args.path_to_chosts;
    let output_path = Path::new(&output_path);
    create_dir_all(output_path)?;

    let client = if let Ok(connect_sid) = env::var("COHOST_COOKIE") {
        info!("COHOST_COOKIE is set; output will include private or logged-in-only chosts!");
        let mut cookie_value = HeaderValue::from_str(&format!("connect.sid={connect_sid}"))?;
        cookie_value.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, cookie_value);
        let client = Client::builder().default_headers(headers).build()?;

        let edited_projects = client
            .get("https://cohost.org/api/v1/trpc/projects.listEditedProjects")
            .send()
            .await?
            .json::<TrpcResponse<ListEditedProjectsResponse>>()
            .await?
            .result
            .data
            .projects;
        let logged_in_project_id = client
            .get("https://cohost.org/api/v1/trpc/login.loggedIn")
            .send()
            .await?
            .json::<TrpcResponse<LoggedInResponse>>()
            .await?
            .result
            .data
            .projectId;
        let logged_in_project = edited_projects
            .iter()
            .find(|project| project.projectId == logged_in_project_id)
            .ok_or_eyre("you seem to be logged in as a project you don’t own")?;
        info!(
            "you are currently logged in as @{}",
            logged_in_project.handle
        );

        if let Some(requested_project) = edited_projects
            .iter()
            .find(|project| project.handle == requested_project)
        {
            if requested_project.projectId != logged_in_project_id {
                bail!(
                    "you wanted to dump chosts for @{}, but you are logged in as @{}",
                    requested_project.handle,
                    logged_in_project.handle,
                );
            } else {
                info!(
                    "dumping chosts for @{}, which you own and are logged in as",
                    requested_project.handle
                );
            }
        } else {
            info!(
                "dumping chosts for @{}, which you don’t own",
                requested_project
            );
        }

        client
    } else {
        info!("COHOST_COOKIE not set; output will exclude private or logged-in-only chosts!");
        Client::builder().build()?
    };

    for page in 0.. {
        let url =
            format!("https://cohost.org/api/v1/project/{requested_project}/posts?page={page}");
        info!("GET {url}");
        let response: PostsResponse = client.get(url).send().await?.json().await?;

        // nItems may be zero if none of the posts on this page are currently visible,
        // but nPages will only be zero when we have run out of pages.
        if response.nPages == 0 {
            break;
        }

        for post_value in response.items {
            let post: Post = serde_json::from_value(post_value.clone())?;
            let path = output_path.join(format!("{}.json", post.postId));
            info!("Writing {path:?}");
            let output_file = File::create(path)?;
            serde_json::to_writer(output_file, &post_value)?;
        }
    }

    Ok(())
}
