use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{Extension, Json};
use password_hash::SaltString;
use rand_core::OsRng;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::entity::user;
use crate::error::{Error, UnauthorizedType};
use crate::session::generate_session;
use crate::user::User;

#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct AuthRequest {
    #[validate(email)]
    email: String,
    #[validate(length(min = 8, max = 64))]
    password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    session: String,
}

pub async fn login(
    Extension(db): Extension<DatabaseConnection>,
    Json(auth): Json<AuthRequest>,
) -> Result<Json<LoginResponse>, Error> {
    auth.validate()?;

    let user = user::Entity::find()
        .filter(user::Column::Email.eq(auth.email.clone()))
        .one(&db)
        .await?;

    let user = match user {
        Some(user) => user,
        None => {
            return Err(Error::Unauthorized(
                UnauthorizedType::WrongUsernameOrPassword,
            ))
        }
    };

    if verify_password(&auth.password, &user.password) {
        let session = generate_session(&db, user.id).await?;
        Ok(Json(LoginResponse { session }))
    } else {
        Err(Error::Unauthorized(
            UnauthorizedType::WrongUsernameOrPassword,
        ))
    }
}

pub async fn profile(user: User) -> Json<User> {
    Json(user)
}

pub async fn register(
    Extension(db): Extension<DatabaseConnection>,
    Json(auth): Json<AuthRequest>,
) -> Result<Json<LoginResponse>, Error> {
    auth.validate()?;

    let count = user::Entity::find()
        .filter(user::Column::Email.eq(auth.email.clone()))
        .count(&db)
        .await?;

    if count > 0 {
        return Err(Error::MustUniqueError("email".to_string()));
    }

    let uuid = Uuid::new_v4();

    let user = user::ActiveModel {
        id: ActiveValue::Set(uuid),
        email: ActiveValue::Set(auth.email),
        password: ActiveValue::Set(hash_password(&auth.password)?),
    };

    user::Entity::insert(user).exec(&db).await?;

    let session = generate_session(&db, uuid).await?;

    Ok(Json(LoginResponse { session }))
}

fn verify_password(password: &str, hashed: &str) -> bool {
    let argon = Argon2::default();

    let hashed = match PasswordHash::new(hashed) {
        Ok(hashed) => hashed,
        Err(_) => return false,
    };

    argon.verify_password(password.as_bytes(), &hashed).is_ok()
}

fn hash_password(password: &str) -> Result<String, Error> {
    let argon = Argon2::default();
    let salt = SaltString::generate(&mut OsRng);

    argon
        .hash_password(password.as_bytes(), &salt)
        .map(|it| it.to_string())
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::MigratorTrait;
    use sea_orm::{Database, DatabaseConnection};

    pub async fn connection() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        db
    }

    #[tokio::test]
    pub async fn test_login() {
        let req = AuthRequest {
            email: "example@example.com".to_string(),
            password: "examplepassword".to_string(),
        };

        let db = Extension(connection().await);

        register(db.clone(), Json(req.clone())).await.unwrap();

        let Json(session) = login(db.clone(), Json(req.clone())).await.unwrap();

        assert!(matches!(
            login(
                db.clone(),
                Json(AuthRequest {
                    password: "wrongpassword".to_string(),
                    ..req.clone()
                }),
            )
            .await,
            Err(Error::Unauthorized(
                UnauthorizedType::WrongUsernameOrPassword
            ))
        ));

        let session = Session {
            bearer: axum_auth::AuthBearer(session.session),
        };

        let Json(user) = profile(crate::user::User::from_session(&db, session).await.unwrap()).await;
        assert_eq!(user.email, req.email);
    }

    #[tokio::test]
    pub async fn test_register() {
        let db = Extension(connection().await);

        let req = AuthRequest {
            email: "example@example.com".to_string(),
            password: "examplepassword".to_string(),
        };
        register(db.clone(), Json(req.clone())).await.unwrap();

        assert!(matches!(
            register(db.clone(), Json(req.clone())).await,
            Err(Error::MustUniqueError(_))
        ));
    }

    #[tokio::test]
    pub async fn test_hash() {
        let password = "examplepassword";
        let hashed = hash_password(password).unwrap();
        assert!(verify_password(password, &hashed));
    }
}
