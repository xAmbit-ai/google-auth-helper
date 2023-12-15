use async_trait::async_trait;
use google_cloud_storage::client::{google_cloud_auth::credentials::CredentialsFile, ClientConfig};
use hyper::client::HttpConnector;
use hyper_rustls::HttpsConnector;
use std::error::Error;
use std::fs::read_to_string;
use std::path::PathBuf;
use yup_oauth2::authenticator::{ApplicationDefaultCredentialsTypes, Authenticator};
use yup_oauth2::authorized_user::AuthorizedUserFlow;
use yup_oauth2::{
    read_authorized_user_secret, ApplicationDefaultCredentialsAuthenticator,
    ApplicationDefaultCredentialsFlowOpts, AuthorizedUserAuthenticator,
    ServiceAccountAuthenticator, ServiceAccountKey,
};

const DEFAULT_CREDENTIALS_FILE: &str = "application_default_credentials.json";

/// A trait for authenticating with google cloud services across different libraries
#[async_trait]
pub trait AuthHelper: Sized {
    /// Authenticate with google cloud services using the default method
    /// In All libraries the authentication goes through almost same following steps:
    /// 1. Check for GOOGLE_APPLICATION_CREDENTIALS or GOOGLE_APPLICATION_CREDENTIALS_JSON env variable
    /// 2. Check for default location of the credentials file which is ~/.config/gcloud/application_default_credentials.json on linux
    /// 3. Check for creds on metadata server
    async fn auth() -> Result<Self, Box<dyn std::error::Error + Send + Sync>>;

    /// Authenticate with google cloud services using a credentials file (service account file)
    async fn auth_with_file(file: String)
        -> Result<Self, Box<dyn std::error::Error + Send + Sync>>;

    /// Authenticate with google cloud services using env variables of the service account credentials
    async fn auth_with_env(env: String) -> Result<Self, Box<dyn std::error::Error + Send + Sync>>;
}

#[async_trait]
impl AuthHelper for ClientConfig {
    async fn auth() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let auth = ClientConfig::default().with_auth().await?;
        Ok(auth)
    }

    async fn auth_with_file(file: String) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let file = CredentialsFile::new_from_file(file).await?;
        let auth = ClientConfig::default().with_credentials(file).await?;
        Ok(auth)
    }

    async fn auth_with_env(env: String) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let string = std::env::var(env)?;
        let file = CredentialsFile::new_from_str(string.as_str()).await?;
        let auth = ClientConfig::default().with_credentials(file).await?;
        Ok(auth)
    }
}

#[async_trait]
impl AuthHelper for Authenticator<HttpsConnector<HttpConnector>> {
    async fn auth() -> Result<Self, Box<dyn Error + Send + Sync>> {
        // from GOOGLE_APPLICATION_CREDENTIALS_JSON env variable
        let env = "GOOGLE_APPLICATION_CREDENTIALS_JSON".to_string();
        let string = std::env::var(env);
        if let Ok(string) = string {
            let secret = serde_json::from_str::<ServiceAccountKey>(&string)?;
            let auth = ServiceAccountAuthenticator::builder(secret)
                .build()
                .await
                .expect(
                "failed to build service account auth from GOOGLE_APPLICATION_CREDENTIALS_JSON env",
            );

            return Ok(auth);
        }

        // from GOOGLE_APPLICATION_CREDENTIALS env variable
        let env = "GOOGLE_APPLICATION_CREDENTIALS".to_string();
        let string = std::env::var(env);
        if let Ok(string) = string {
            let auth = Authenticator::auth_with_file(string).await.expect(
                "failed to build service account auth from GOOGLE_APPLICATION_CREDENTIALS env",
            );

            return Ok(auth);
        }

        let path: Option<PathBuf> = if cfg!(target_os = "windows") {
            let app_data = std::env::var("APPDATA")?;
            Some(
                std::path::Path::new(&app_data)
                    .join("gcloud")
                    .join(DEFAULT_CREDENTIALS_FILE),
            )
        } else {
            match home::home_dir() {
                Some(s) => Some(
                    s.join(".config")
                        .join("gcloud")
                        .join(DEFAULT_CREDENTIALS_FILE),
                ),
                None => None,
            }
        };

        // check if the file exists
        if let Some(path) = path {
            if path.exists() {
                let auth = read_authorized_user_secret(path).await?;
                let auth = AuthorizedUserAuthenticator::builder(auth)
                    .build()
                    .await
                    .expect("failed to build authorized user auth");

                return Ok(auth);
            }
        }

        let opts = ApplicationDefaultCredentialsFlowOpts::default();

        // some extra time wasted checking for GOOGLE_APPLICATION_CREDENTIALS again for sake of order
        let auth = match ApplicationDefaultCredentialsAuthenticator::builder(opts).await {
            ApplicationDefaultCredentialsTypes::ServiceAccount(auth) => auth
                .build()
                .await
                .expect("failed to build service account auth from env"),
            ApplicationDefaultCredentialsTypes::InstanceMetadata(auth) => auth
                .build()
                .await
                .expect("failed to build instance metadata auth"),
        };
        Ok(auth)
    }

    async fn auth_with_file(file: String) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let secret = serde_json::from_str::<ServiceAccountKey>(&read_to_string(file)?)?;
        let auth = ServiceAccountAuthenticator::builder(secret).build().await?;
        Ok(auth)
    }

    async fn auth_with_env(env: String) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let string = std::env::var(env)?;
        let secret = serde_json::from_str::<ServiceAccountKey>(&string)?;
        let auth = ServiceAccountAuthenticator::builder(secret).build().await?;
        Ok(auth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_cloud_storage::client::{Client, ClientConfig};
    use google_cloudtasks2::{
        api::{CreateTaskRequest, HttpRequest, OidcToken, Task},
        hyper::{client::HttpConnector, Client as HyperClient},
        hyper_rustls::{HttpsConnector, HttpsConnectorBuilder},
        oauth2::authenticator::Authenticator,
        CloudTasks,
    };

    // just creates auth for storage and cloud tasks and does nothing
    #[tokio::test]
    async fn test_auth() {
        let storage = ClientConfig::auth().await.unwrap();
        let client = Client::new(storage);

        let auth = Authenticator::auth().await.unwrap();
        let hub = CloudTasks::new(
            HyperClient::builder().build(
                HttpsConnectorBuilder::new()
                    .with_native_roots()
                    .https_only()
                    .enable_http1()
                    .enable_http2()
                    .build(),
            ),
            auth,
        );
    }

    // creates a task in cloud tasks
    #[tokio::test]
    async fn cloud_tasks_test() {
        let auth = Authenticator::auth().await.unwrap();
        let hub = CloudTasks::new(
            HyperClient::builder().build(
                HttpsConnectorBuilder::new()
                    .with_native_roots()
                    .https_only()
                    .enable_http1()
                    .enable_http2()
                    .build(),
            ),
            auth,
        );

        let mut task = Task::default();
        let http_request = HttpRequest {
            http_method: Some("GET".to_string()),
            url: Some("https://jsonplaceholder.typicode.com/posts/1".to_string()),
            ..Default::default()
        };
        task.http_request = Some(http_request);

        let rq = CreateTaskRequest {
            response_view: None,
            task: Some(task),
        };

        let queue_path = std::env::var("QUEUE_PATH").expect("QUEUE_PATH env variable not set");

        let (r, t) = hub
            .projects()
            .locations_queues_tasks_create(rq, queue_path.as_str())
            .doit()
            .await
            .unwrap();

        println!("{:?}", r);
        println!("{:?}", t);
    }
}
