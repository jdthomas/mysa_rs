// From: https://github.com/hypnoseal/cognitoidentityprovider-examples
#[path = "mqtt_memory_persist.rs"]
mod mqtt_memory_persist;
use std::time::Duration;

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use aws_config::meta::region::RegionProviderChain;
use aws_config::retry::RetryConfig;
use aws_sdk_cognitoidentityprovider::config::Region;
use aws_sdk_cognitoidentityprovider::types::AuthFlowType;
use aws_sdk_cognitoidentityprovider::types::AuthenticationResultType;
use aws_sdk_cognitoidentityprovider::types::ChallengeNameType;
use cognito_srp::SrpClient;
use futures::StreamExt;
use tokio_stream::wrappers::IntervalStream;

struct MysaAuth {
    pool_id: String,
    client_id: String,
    region: String,
    username: String,
    password: String,
}

impl MysaAuth {
    fn try_from_env() -> Result<Self> {
        Ok(Self {
            // Pilfered login info from one of:
            //      https://github.com/drinkwater99/MySa/blob/master/Program.cs
            //      https://github.com/fdurand/mysa-thermostats/blob/master/main.go
            pool_id: "us-east-1_GUFWfhI7g".to_owned(),
            client_id: "19efs8tgqe942atbqmot5m36t3".to_owned(),
            region: "us-east-1".to_owned(),
            username: std::env::var("MYSA_USERNAME").context("MYSA_USERNAME not found")?,
            password: std::env::var("MYSA_PASSWORD").context("MYSA_PASSWORD not found")?,
        })
    }
}

struct MqttConfig {
    host: String,
    client_id: String,
    topic_prefix: String,
}

impl MqttConfig {
    fn try_from_env() -> Result<Self> {
        const DEVICE_NAME: &str = "mysa";
        Ok(Self {
            host: std::env::var("MQTT_HOST").context("MQTT_HOST not set")?,
            client_id: "mysa_rs.service".to_owned(),
            topic_prefix: format!(
                "{}/{}",
                std::env::var("MQTT_PREFIX").context("MQTT_PREFIX not set")?,
                DEVICE_NAME
            ),
        })
    }
}

fn get_mqtt_client(mqtt_config: &MqttConfig) -> Result<paho_mqtt::Client> {
    // Define the set of options for the create.
    // Use an ID for a persistent session.
    let create_opts = paho_mqtt::CreateOptionsBuilder::new()
        .server_uri(&mqtt_config.host)
        .client_id(&mqtt_config.client_id)
        .user_persistence(mqtt_memory_persist::MemPersistence::new())
        .finalize();

    // Create a client.
    let cli = paho_mqtt::Client::new(create_opts).context("mqtt client create")?;

    // Define the set of options for the connection.
    let conn_opts = paho_mqtt::ConnectOptionsBuilder::new()
        .keep_alive_interval(Duration::from_secs(20))
        .clean_session(true)
        .finalize();

    // Connect and wait for it to complete or fail.
    cli.connect(conn_opts).context("mqtt client connect")?;
    Ok(cli)
}

async fn poll_mysa_and_report_to_mqtt() -> Result<()> {
    const POLLING_PERIOD_SEC: u64 = 15 * 60;

    let mut polling_interval = IntervalStream::new(tokio::time::interval(
        std::time::Duration::from_secs(POLLING_PERIOD_SEC),
    ))
    .fuse();

    // Get env variables
    let mysa_device_id = std::env::var("MYSA_DEVICE_ID").context("MYSA_DEVICE_ID not set")?;

    let mysa_auth_params =
        MysaAuth::try_from_env().context("Problem getting auth params from env")?;
    let mut tokens = get_tokens(&mysa_auth_params, None).await?;
    let mut token_refresh_interval = IntervalStream::new(tokio::time::interval(
        std::time::Duration::from_secs(tokens.expires_in() as u64 - 30),
    ))
    .fuse();
    let _ = token_refresh_interval.next(); // Skip the initial one, we already grabbed the tokens

    let mqtt_config = MqttConfig::try_from_env()?;

    let mqtt_client = get_mqtt_client(&mqtt_config)?;

    loop {
        tokio::select! {
            _ = token_refresh_interval.select_next_some() => {
                // FIXME: Do we ever get other expirations, does this interval need to update?
                println!("tokens expire every {} seconds. Updating now", tokens.expires_in());
                tokens = get_tokens(&mysa_auth_params, Some(tokens)).await?;
            },
            _ = polling_interval.select_next_some() =>
            {
                println!("...");

                let reading = get_mysa_reading(tokens.id_token.as_ref().unwrap(), &mysa_device_id).await?;

                // And the mqtt part
                send_to_mqtt(&mqtt_client, &mqtt_config, reading)?;

            },
        }
    }
}

async fn refresh_tokens(
    cognito_client: &aws_sdk_cognitoidentityprovider::Client,
    auth_params: &MysaAuth,
    current_tokens: &AuthenticationResultType,
) -> Result<AuthenticationResultType> {
    if let Some(refresh_token) = &current_tokens.refresh_token {
        println!("Trying token refresh...");
        let params = maplit::hashmap! {
            "REFRESH_TOKEN".to_owned() => refresh_token.clone()
        };
        let auth_init_res = cognito_client
            .initiate_auth()
            .auth_flow(AuthFlowType::RefreshTokenAuth)
            .client_id(&auth_params.client_id)
            .set_auth_parameters(Some(params))
            .send()
            .await
            .context("Failed to refresh");
        if let Ok(auth_init_res) = auth_init_res {
            if let Some(refreshed_tokens) = auth_init_res.authentication_result() {
                println!("(refreshed) Auth tokens: {:?}", refreshed_tokens);
                return Ok(refreshed_tokens.clone());
            }
        }
        Err(anyhow!("Failed token refresh, continue to login"))
    } else {
        Err(anyhow!("No refresh token"))
    }
}

async fn get_tokens(
    auth_params: &MysaAuth,
    current_tokens: Option<AuthenticationResultType>,
) -> Result<AuthenticationResultType> {
    // Set AWS region and config.
    let region_provider = RegionProviderChain::first_try(Region::new(auth_params.region.clone()))
        .or_default_provider();
    let shared_config = aws_config::from_env().region(region_provider).load().await;
    let config = aws_sdk_cognitoidentityprovider::config::Builder::from(&shared_config)
        .retry_config(RetryConfig::disabled())
        .build();
    let cognito_client = aws_sdk_cognitoidentityprovider::Client::from_conf(config);

    if let Ok(refreshed_tokens) = match &current_tokens {
        Some(ct) => refresh_tokens(&cognito_client, auth_params, ct).await,
        None => Err(anyhow!("meh")),
    } {
        return Ok(refreshed_tokens);
    }

    let srp_client = SrpClient::new(
        &auth_params.username,
        &auth_params.password,
        &auth_params.pool_id,
        &auth_params.client_id,
        None,
    );
    let params = srp_client.get_auth_params().context("params").unwrap();
    let auth_init_res = cognito_client
        .initiate_auth()
        .auth_flow(AuthFlowType::UserSrpAuth)
        .client_id(&auth_params.client_id)
        .set_auth_parameters(Some(params))
        .send()
        .await;

    let auth_init_out = auth_init_res.context("result").unwrap();
    if auth_init_out.challenge_name.is_none()
        || auth_init_out.challenge_name.clone().unwrap() != ChallengeNameType::PasswordVerifier
    {
        if let Some(cn) = auth_init_out.challenge_name {
            println!("challenge_name is unexpected, got {:?}", cn);
        } else {
            println!("No challenge found in init");
        }
    }

    let challenge_params = auth_init_out
        .challenge_parameters
        .context("No challenge was returned for the client")?;
    let challenge_responses = srp_client.process_challenge(challenge_params)?;

    let password_challenge_res = cognito_client
        .respond_to_auth_challenge()
        .set_challenge_responses(Some(challenge_responses))
        .client_id(auth_params.client_id.clone())
        .challenge_name(ChallengeNameType::PasswordVerifier)
        .send()
        .await?;

    let auth_res: Result<Option<AuthenticationResultType>> =
        match password_challenge_res.challenge_name {
            Some(ChallengeNameType::SoftwareTokenMfa) | Some(ChallengeNameType::SmsMfa) => {
                todo!("not supported")
            }
            Some(_) | None => Ok(password_challenge_res.authentication_result),
        };

    let tokens = auth_res.context("unable to get tokens")?.unwrap();
    println!("Auth tokens: {:?}", tokens);

    if let Ok(refreshed_tokens) = refresh_tokens(&cognito_client, auth_params, &tokens).await {
        return Ok(refreshed_tokens);
    }

    Ok(tokens)
}

#[allow(dead_code)]
struct MysaReading {
    // TODO: types
    temp_c: String,
    humidity_pct: String,
    timestamp: String,
    set_point_c: String,
}

async fn get_mysa_reading(id_token: &str, mysa_device_id: &str) -> Result<MysaReading> {
    const MYSA_DATA_URL: &str = "https://app-prod.mysa.cloud/users/readingsForUser";

    let client = reqwest::Client::new();
    let res = client
        .get(MYSA_DATA_URL)
        .header("Authorization", format!("Bearer {}", id_token))
        .send()
        .await?;
    println!("res: {:?}", res);
    let json_response = res.json::<serde_json::Value>().await?;
    println!("res: {:#?}", json_response);

    // FIXME: Probably could ignore teh device_id and just return a vec of any found
    let reading = &json_response["devices"][mysa_device_id]["Reading"];
    Ok(MysaReading {
        temp_c: reading["MainTemp"].to_string(),
        humidity_pct: reading["Humidity"].to_string(),
        timestamp: reading["Timestamp"].to_string(),
        set_point_c: reading["SetPoint"].to_string(),
    })
}

fn send_to_mqtt(
    mqtt_client: &paho_mqtt::Client,
    mqtt_config: &MqttConfig,
    reading: MysaReading,
) -> Result<()> {
    const QOS: i32 = 1;

    let msgs = &[
        paho_mqtt::Message::new(
            format!("{}/{}", mqtt_config.topic_prefix, "set_point/c"),
            reading.set_point_c,
            QOS,
        ),
        paho_mqtt::Message::new(
            format!("{}/{}", mqtt_config.topic_prefix, "temperature/c"),
            reading.temp_c,
            QOS,
        ),
        paho_mqtt::Message::new(
            format!("{}/{}", mqtt_config.topic_prefix, "humidity/pct"),
            reading.humidity_pct,
            QOS,
        ),
    ];
    let toks: Result<Vec<()>> = msgs
        .iter()
        .map(|msg| {
            mqtt_client
                .publish(msg.clone())
                .map_err(anyhow::Error::from)
        })
        .collect();
    toks.context("mqtt publish")?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("MYSA to MQTT ...");
    poll_mysa_and_report_to_mqtt().await?;

    Ok(())
}
