use bollard::container::{
    Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, PortBinding};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::path::Path;

const CONTAINER_NAME: &str = "tern_shadow_db";

fn get_docker_client(engine: &str) -> Result<Docker, Box<dyn std::error::Error>> {
    match engine.to_lowercase().as_str() {
        "podman" => {
            if let Ok(home) = std::env::var("HOME") {
                let podman_mac_sock = format!(
                    "{}/.local/share/containers/podman/machine/podman-machine-default/api.sock",
                    home
                );
                if Path::new(&podman_mac_sock).exists() {
                    return Ok(Docker::connect_with_unix(
                        &podman_mac_sock,
                        120,
                        bollard::API_DEFAULT_VERSION,
                    )?);
                }
            }
            if let Ok(xrd) = std::env::var("XDG_RUNTIME_DIR") {
                let sock = format!("{}/podman/podman.sock", xrd);
                if Path::new(&sock).exists() {
                    return Ok(Docker::connect_with_unix(
                        &sock,
                        120,
                        bollard::API_DEFAULT_VERSION,
                    )?);
                }
            }
            Ok(Docker::connect_with_unix_defaults()?)
        }
        _ => {
            if let Ok(home) = std::env::var("HOME") {
                let docker_mac_sock = format!("{}/.docker/run/docker.sock", home);
                if !Path::new("/var/run/docker.sock").exists()
                    && Path::new(&docker_mac_sock).exists()
                {
                    return Ok(Docker::connect_with_unix(
                        &docker_mac_sock,
                        120,
                        bollard::API_DEFAULT_VERSION,
                    )?);
                }
            }
            Ok(Docker::connect_with_socket_defaults()?)
        }
    }
}

pub async fn provision_container(
    engine: &str,
    db_type: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_type = db_type.to_lowercase();

    // SQLite does not need a container daemon
    if db_type == "sqlite" {
        println!("-> SQLite selected. Skipping container provisioning...");
        return Ok(());
    }

    let docker = get_docker_client(engine)?;

    // In src/engine.rs
    let (image, port, env) = match db_type.as_str() {
        "mysql" => (
            "mysql:8",
            "3306/tcp",
            vec![
                "MYSQL_ROOT_PASSWORD=tern_password",
                "MYSQL_DATABASE=tern_db",
            ],
        ),
        _ => (
            "postgres:latest",
            "5432/tcp",
            vec!["POSTGRES_PASSWORD=tern_password"],
        ),
    };

    println!("-> Pulling image '{}' via {}...", image, engine);

    let mut stream = docker.create_image(
        Some(CreateImageOptions {
            from_image: image,
            ..Default::default()
        }),
        None,
        None,
    );

    // Consume the stream to ensure the image is fully pulled before proceeding
    while let Some(msg) = stream.next().await {
        if let Err(e) = msg {
            eprintln!("Warning during image pull: {}", e);
        }
    }

    // Ensure no lingering container from a previous run exists
    let _ = teardown_container(engine).await;

    println!("-> Creating container '{}'...", CONTAINER_NAME);

    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        port.to_string(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(port.split('/').next().unwrap().to_string()),
        }]),
    );

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        ..Default::default()
    };

    let config = Config {
        image: Some(image.to_string()),
        env: Some(env.iter().map(|s| s.to_string()).collect()),
        host_config: Some(host_config),
        ..Default::default()
    };

    docker
        .create_container(
            Some(CreateContainerOptions {
                name: CONTAINER_NAME,
                platform: None,
            }),
            config,
        )
        .await?;

    println!(
        "-> Starting container '{}' on port {}...",
        CONTAINER_NAME, port
    );

    docker
        .start_container(CONTAINER_NAME, None::<StartContainerOptions<String>>)
        .await?;

    Ok(())
}

pub async fn teardown_container(engine: &str) -> Result<(), Box<dyn std::error::Error>> {
    let docker = get_docker_client(engine)?;
    // Using force: true will automatically stop and remove the container
    let options = RemoveContainerOptions {
        force: true,
        v: true, // Remove associated volumes
        ..Default::default()
    };

    match docker.remove_container(CONTAINER_NAME, Some(options)).await {
        Ok(_) => println!("-> Teardown complete: {} removed.", CONTAINER_NAME),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(e) => {
            if !e.to_string().contains("No such container") {
                eprintln!("-> Note during teardown: {}", e);
            }
        }
    }

    Ok(())
}
