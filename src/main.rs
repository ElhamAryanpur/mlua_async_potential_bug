use mlua::{Lua, LuaSerdeExt};
use sqlx::Executor;

static DATABASE: once_cell::sync::OnceCell<Database> = once_cell::sync::OnceCell::new();
static TOKIO_HANDLE: once_cell::sync::Lazy<tokio::sync::Mutex<Option<tokio::runtime::Handle>>> =
    once_cell::sync::Lazy::new(Default::default);

#[derive(Debug)]
pub struct Database {
    pub pool: sqlx::Pool<sqlx::Postgres>,
}
impl Database {
    pub async fn new() -> Result<Self, sqlx::Error> {
        // Connect to the database.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect("postgres://postgres:password@localhost/database")
            .await?;

        if let Err(e) = DATABASE.set(Self { pool: pool.clone() }) {
            println!("DATABASE ERROR: {e:#?}");
        }

        pool.execute(
            "CREATE TABLE IF NOT EXISTS test (id UUID PRIMARY KEY, key TEXT UNIQUE, value TEXT);",
        )
        .await?;

        Ok(Self { pool })
    }
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
struct DatabaseType {
    id: uuid::Uuid,
    key: String,
    value: String,
}

// doesn't run at all as needs tokio
pub async fn call_service(lua: &mlua::Lua, key: String) -> Result<mlua::Value, mlua::Error> {
    let database = DATABASE.get().unwrap();

    let result =
        match sqlx::query_as::<_, DatabaseType>("SELECT * FROM key_value WHERE key = $1 LIMIT 1;")
            .bind(key)
            .fetch_optional(&database.pool)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                println!("error getting data storage from database: {e:#?}");
                None
            }
        };

    if let Some(result) = result {
        lua.to_value(&result)
    } else {
        Ok(mlua::Value::Nil)
    }
}

pub async fn call_service_with_handle(
    lua: &mlua::Lua,
    key: String,
) -> Result<mlua::Value, mlua::Error> {
    let database = DATABASE.get().unwrap();
    let handle = TOKIO_HANDLE.lock().await;
    let handle = handle.as_ref().unwrap();

    // spawn a tokio thread
    let result = handle
        .spawn(async move {
            match sqlx::query_as::<_, DatabaseType>("SELECT * FROM test WHERE key = $1 LIMIT 1;")
                .bind(key)
                .fetch_optional(&database.pool)
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    println!("error getting data storage from database: {e:#?}");
                    None
                }
            }
        })
        .await
        .expect("couldn't execute the query");

    if let Some(result) = result {
        lua.to_value(&result)
    } else {
        Ok(mlua::Value::Nil)
    }
}

#[tokio::main]
async fn main() {
    let _ = Database::new().await.unwrap();
    *TOKIO_HANDLE.lock().await = Some(tokio::runtime::Handle::current());

    // Originally I'm running this on a server
    // using a threrad to communicate to the vm as to not block the server
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let lua = Lua::new();

        // without tokio handler
        lua.globals()
            .set(
                "call_service",
                lua.create_async_function(move |lua, key: String| async {
                    let result = call_service(lua, key).await;
                    println!("{result:#?}");

                    result
                })
                .expect("couldn't create the function"),
            )
            .expect("couldn't add the function to global");

        // using tokio handler
        lua.globals()
            .set(
                "call_service_handle",
                lua.create_async_function(move |lua, key: String| async {
                    // no result is returned
                    let result = call_service_with_handle(lua, key).await;
                    println!("{result:#?}");

                    result
                })
                .expect("couldn't create the function"),
            )
            .expect("couldn't add the function to global");

        if let Err(e) = lua.load(include_str!("../program.lua")).exec() {
            /*
               One of the errors I'm getting with this setup:

               Error when executing the program: runtime error: attempt to yield across metamethod/C-call boundary
               stack traceback:
                       [C]: in ?
                       [C]: in function 'yield'
                       [string "__mlua_async_poll"]:16: in function 'call_service_handle'
                       [string "src/main.rs:125:29"]:1: in ?
            */
            eprintln!("Error when executing the program: {e}");
        }
    });

    // for the thread to run.
    std::thread::sleep(std::time::Duration::from_secs(60));
}
