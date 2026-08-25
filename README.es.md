<div align="center">

<img src="./logo.jpg" alt="KeyCompute logo" width="160" style="border-radius: 20px;" />

# KeyCompute

<p align="center">
  <a href="./README.zh-CN.md">简体中文</a> |
  <a href="./README.md">English</a> |
  <a href="./README.zh-TW.md">繁體中文</a> |
  <a href="./README.es.md">Español</a> |
  <a href="./README.ar.md">العربية</a>
</p>

**Plataforma de servicios de cómputo de tokens de IA de nueva generación y alto rendimiento**

<p align="center">
  <a href="https://github.com/keycompute/keycompute/stargazers"><img src="https://img.shields.io/github/stars/keycompute/keycompute?style=social" alt="GitHub Stars" /></a>
  <a href="https://github.com/keycompute/keycompute/issues"><img src="https://img.shields.io/github/issues/keycompute/keycompute" alt="GitHub Issues" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License" /></a>
  <a href="./CONTRIBUTING.md"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs Welcome" /></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Rust-1.92%2B-orange?logo=rust" alt="Rust Version" /></a>
</p>

<p align="center">
  <a href="#características">Características</a> •
  <a href="#arquitectura">Arquitectura</a> •
  <a href="#inicio-rápido">Inicio rápido</a> •
  <a href="#configuración">Configuración</a> •
  <a href="#estructura-del-proyecto">Estructura del proyecto</a> •
  <a href="#api">API</a> •
  <a href="#guía-de-desarrollo">Desarrollo</a>
</p>

</div>

---

## Descripción general

KeyCompute es una plataforma de servicios de cómputo de tokens de IA **de alto rendimiento**, **extensible** y **lista para usar**, que proporciona capacidades de nivel empresarial como acceso unificado a LLMs, enrutamiento inteligente, medición y facturación, arrendamiento de nodos de cómputo, distribución multinivel y observabilidad.

> **Pure Rust Full Stack**: Backend (Axum) + Frontend (Dioxus WASM) + CLI cliente, tipos y lógica compartidos, máximo rendimiento y seguridad.

> **Nota**: Este proyecto es solo para aprendizaje personal. Debe utilizarse de acuerdo con los [Términos de uso](https://openai.com/policies) de OpenAI y con las leyes y normativas aplicables. No lo utilice para fines ilegales. De conformidad con las Medidas Provisionales para la Administración de Servicios de Inteligencia Artificial Generativa, no proporcione servicios de IA generativa no registrados al público en China.

---

## Características

### Arrendamiento de nodos de cómputo
Los nodos de cómputo se conectan mediante **sondeo pull-based** sin necesidad de **IP pública**. Ejecutan modelos alojados en hardware local y obtienen recompensas según sus contribuciones.

- **Conexión con un clic**: ejecuta el binario CLI independiente para auto-registro → heartbeat → sondeo de tareas → ejecución local → envío de resultados
- **Enrutamiento de nodos**: usa `node:<nombre_modelo>` para enrutar solicitudes explícitamente al pool de nodos
- **Conmutación por error automática**: los nodos fallidos se excluyen, las tareas se reencolan automáticamente
- **Persistencia de sesión**: las sesiones locales evitan registros duplicados; el cierre graceful garantiza la integridad de las tareas
- **Mecanismo de propinas**: los propietarios de nodos pueden ganar y retirar propinas

### Gateway multimodelo unificado
Cambia sin problemas entre todos los modelos principales con la **API OpenAI** estándar — solo una línea de código:

| Provider | Familias de modelos | Implementación |
|:---|:---|:---:|
| 🟢 OpenAI | GPT-4o / GPT-4 / GPT-3.5 etc. | ✅ |
| 🟣 Anthropic | Claude 3.5 Sonnet / Opus / Haiku etc. | ✅ |
| 🔵 Google | Gemini 1.5 / 2.0 Flash / Pro etc. | ✅ |
| 🔴 DeepSeek | DeepSeek-V3 / R1 / Chat etc. | ✅ |
| 🟤 Ollama | Modelos locales (Llama / Qwen / GLM / MiniMax etc.) | ✅ |
| 🟡 vLLM | Modelos autohospedados | ✅ |

> GLM (Zhipu) y MiniMax se pueden implementar localmente mediante el adaptador Ollama, no como implementaciones de Provider independientes.

### Motor de enrutamiento inteligente
**Arquitectura de enrutamiento de dos capas** con puntuación ponderada multifactor para una selección óptima:

```text
puntuación = 0.30 × Factor de costo + 0.25 × Factor de latencia + 0.25 × Tasa de éxito + 0.20 × Estado de salud
```

- **Enrutamiento a nivel de modelo** → **Enrutamiento de pool de cuentas**: distribuye automáticamente entre providers y cuentas
- **Cadena de respaldo**: cambia automáticamente a objetivos de respaldo cuando falla el principal
- **Reintento con backoff exponencial**: hasta 3 reintentos, 100ms inicial, 10s máximo
- **Proxy a nivel de solicitud**: soporta proxies HTTP a nivel de provider / cuenta / comodín

### Sistema de facturación y pagos

- **Liquidación posterior a la transmisión**: cálculo preciso después de completar la solicitud, sin deducción previa, sin impacto en los resultados
- **Precios de tres niveles**: precio específico del inquilino → Predeterminado de BD → Respaldo codificado (caché LRU)
- **Uso preciso**: prioridad al uso preciso del provider, retrocede a estimación tiktoken
- **Recarga en línea**: Alipay/WeChat Pay + gestión de saldo
- **Analítica de uso**: desglose detallado del consumo de tokens con visualización

### Sistema de distribución por referidos

- **Comisiones por recomendación**: 3% predeterminado para primer nivel + 2% para segundo nivel, cálculo automático
- **Enlaces de invitación**: genera enlaces exclusivos con un clic
- **Configuración flexible**: los administradores configuran las proporciones de distribución mediante API
- **Analítica de ingresos**: consulta ganancias y lista de referidos en tiempo real

### Autenticación y permisos

- **Autenticación dual**: JWT (sesiones de usuario) + API Key (`sk-...`, acceso API)
- **Separación de permisos**: una API Key con rol de admin no puede acceder a la interfaz de administración
- **Gestión completa de usuarios**: Registro → Verificación de correo → Inicio de sesión → Restablecimiento de contraseña → Gestión de roles
- **Limitación por grupos**: limitación a nivel de usuario / inquilino / API Key (backend dual memoria/Redis)

### Observabilidad

- **Métricas de Prometheus**: volumen de solicitudes, latencia, tasa de error, salud del provider
- **Trazabilidad distribuida**: Provider Span / Request Span / Stream Span
- **Logs estructurados**: formato JSON, salida por niveles desarrollo/producción
- **Monitorización de host**: métricas en tiempo real de CPU / Memoria / Disco / Red
- **Endpoint de salud**: `/health` para monitorización del estado del servicio con un clic

### Frontend multiplataforma

- **Panel de administración web**: Dioxus WASM SPA
- **Escritorio**: aplicación nativa Dioxus Desktop
- **Móvil**: soporte multiplataforma Dioxus Mobile
- **Control de permisos a nivel de ruta**: verificación de rol Admin, seguro y manejable

---

## Arquitectura

```text
[Cliente: Web / Desktop / Mobile (Dioxus)]
                ↕ HTTP/SSE
[Capa API: keycompute-server (Axum)]
       ├── Autenticación (JWT + API Key)
       ├── Limitación (Memoria/Redis)
       ├── Enrutamiento (Motor de dos capas)
       └── Gateway (Única capa de ejecución upstream)
                ↕
[Capa de adaptadores de Provider]
  ├── OpenAI / Anthropic / Google
  ├── DeepSeek
  ├── Ollama (Modelos locales)
  └── vLLM (Autohospedados)

[Red de nodos de cómputo]
  node-token (CLI) ↔ node-gateway ↔ Cola de tareas Redis ↔ Inferencia local
```

---

## Inicio rápido

### Requisitos

| Componente | Versión requerida |
|:---|:---|
| Rust | ≥ 1.92 |
| Axum | ≥ 0.8.0 |
| Dioxus | ≥ 0.7.1 (desarrollo frontend) |
| PostgreSQL | ≥ 16 |
| Redis | ≥ 7 (opcional, para limitación distribuida/cola de nodos) |
| Docker | Última versión (despliegue en contenedores) |

### Opción 1: despliegue con Docker Compose (recomendado)

```bash
# Clonar el proyecto
git clone https://github.com/keycompute/keycompute.git
cd keycompute

# Copiar y editar las variables de entorno
cp .env.example .env
# Antes del primer arranque en producción, sustituye todas las credenciales.
# La aplicación exige como mínimo:
# - KC__AUTH__JWT_SECRET: no predeterminado, no vacío, al menos 32 bytes
# - KC__CRYPTO__SECRET_KEY: Base64 de exactamente 32 bytes
# - KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET: no predeterminado, al menos 16 bytes
#   (obligatorio aquí porque este Compose habilita Redis/Node Gateway)
# - KC__DEFAULT_ADMIN_PASSWORD: no predeterminado, no vacío, al menos 12 caracteres
#   (solo al crear el primer administrador system)

# Iniciar todos los servicios
docker compose up -d

# Comprobar el estado de los servicios
docker compose ps
```

Después del despliegue, abre `http://localhost` (o el puerto definido en `WEB_PORT`).

Si no se cambia, el correo del administrador inicial es `admin@keycompute.local`.
La contraseña es el valor configurado en `KC__DEFAULT_ADMIN_PASSWORD`; una base
de datos de producción nueva nunca acepta `change-me-admin-password`. Después de
crear el primer administrador `system`, elimina esta contraseña de un solo uso
del entorno; los reinicios posteriores no la necesitan.

### Opción 2: desarrollo local

> ⚠️ **Advertencia de seguridad**: Los valores predeterminados que se muestran a continuación (`change-me-*`) son solo para demostración.
> **¡Nunca los uses en producción!** Genera contraseñas aleatorias seguras usando:
> ```bash
> openssl rand -base64 32
> ```
> Un `cargo run` normal compila el ejecutable debug, por lo que el propio método
> de inicio selecciona el modo de desarrollo. Solo lee `config.toml`, permite
> credenciales locales públicas y puede escuchar en `0.0.0.0`. `.env.example`,
> `KC__*` y `APP_BASE_URL` no sobrescriben ese archivo; no lo uses en producción.

```bash
# Crear la configuración usada por cargo run
cp config.example.toml config.toml

# Iniciar PostgreSQL y Redis con puertos locales que coinciden con config.toml
docker compose --env-file .env.example -f docker-compose.yml -f docker-compose.dev.yml up -d postgres redis

# Instalar dioxus-cli
curl -sSL http://dioxus.dev/install.sh | sh

# Iniciar el backend
cargo run -p keycompute-server

# Iniciar el servidor de desarrollo frontend (en otra terminal)
dx serve --package web --platform web --hot-reload true --addr 0.0.0.0
```

`cargo run -p keycompute-server --release` y la imagen Docker seleccionan el
modo de producción. El ejecutable release solo lee variables de entorno e ignora
`config.toml`; ningún parámetro ni variable de entorno cambia el modo de ejecución.

---

## Estructura del proyecto

```text
keycompute/
├── crates/                          # Módulos principales del backend (Rust)
│   ├── keycompute-server/            # Servicio HTTP Axum (integra todos los módulos)
│   ├── keycompute-types/             # Tipos y macros compartidos
│   ├── keycompute-db/                # ORM y migraciones de base de datos
│   ├── keycompute-auth/              # Autenticación y autorización (JWT + API Key + Contraseña)
│   ├── keycompute-cache/             # Abstracción de caché y soporte Redis
│   ├── keycompute-ratelimit/         # Motor de limitación (backend dual memoria/Redis)
│   ├── keycompute-pricing/           # Motor de precios (tres niveles + caché LRU)
│   ├── keycompute-routing/           # Motor de enrutamiento inteligente de dos capas
│   ├── keycompute-runtime/           # Tiempo de ejecución (cifrado AES-256-GCM + abstracción de almacenamiento)
│   ├── keycompute-billing/           # Facturación y liquidación (liquidación precisa post-transmisión)
│   ├── keycompute-distribution/      # Sistema de distribución por referidos
│   ├── keycompute-observability/     # Tres pilares de observabilidad
│   ├── keycompute-config/            # Cargadores separados: TOML debug / entorno release
│   ├── keycompute-emailserver/       # Servicio de correo SMTP
│   ├── keycompute-payment/           # Integración de pagos
│   │   ├── keycompute-alipay/        # Pago Alipay
│   │   └── keycompute-wechatpay/     # Pago WeChat
│   ├── llm-gateway/                  # Gateway de ejecución LLM (única capa upstream)
│   ├── llm-protocol/                 # Traducción de protocolos y definiciones de providers
│   │   ├── openai/                   # Protocolo compatible con OpenAI
│   │   ├── anthropic/                # Protocolo compatible con Anthropic
│   │   └── provider/                 # Tipos de protocolo compartidos
│   ├── node-gateway/                 # Gateway de nodos (registro/heartbeat/gestión de tareas)
│   └── integration-tests/           # Pruebas de integración integrales
├── packages/                         # Frontend (Dioxus 0.7)
│   ├── web/                          # Panel de administración web
│   ├── ui/                           # Biblioteca de componentes UI compartidos
│   ├── desktop/                      # Aplicación nativa de escritorio
│   ├── mobile/                       # Aplicación multiplataforma móvil
│   └── client-api/                   # Cliente API encapsulado
├── nginx/                            # Configuración de proxy inverso Nginx
├── Dockerfile.server                 # Imagen del backend
├── Dockerfile.web                    # Imagen del frontend
├── docker-compose.yml                # Orquestación de producción
├── docker-compose.dev.yml            # Puertos locales para dependencias
└── docker-compose.replicas.yml       # Orquestación con réplicas de lectura
```

---

## Configuración

### Variables de entorno

Esta tabla se aplica al inicio de producción release/Docker. Un `cargo run`
debug usa `config.toml`.

| Variable | Descripción | Obligatoria |
|:---|:---|:---:|
| `KC__DATABASE__URL` | Cadena de conexión de PostgreSQL | ✅ |
| `KC__AUTH__JWT_SECRET` | Producción: secreto JWT no predeterminado/no vacío de al menos 32 bytes | ✅ |
| `KC__CRYPTO__SECRET_KEY` | Producción: Base64 de exactamente 32 bytes; no puede cambiarse después de guardar API keys de Provider | ✅ |
| `KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET` | Producción con Redis: secreto HMAC no predeterminado de al menos 16 bytes; emite tokens de registro de un solo uso | Condicional |
| `KC__REDIS__URL` | Cadena de conexión de Redis (opcional; sin ella: limitador de tasa cae a memoria, caché pasa a no-op, puerta de enlace de nodos no disponible) | ⚪ |
| `KC__EMAIL__SMTP_HOST` | Host SMTP (opcional) | ⚪ |
| `KC__EMAIL__SMTP_PORT` | Puerto SMTP (opcional) | ⚪ |
| `KC__EMAIL__SMTP_USERNAME` | Usuario SMTP (opcional) | ⚪ |
| `KC__EMAIL__SMTP_PASSWORD` | Contraseña SMTP (opcional) | ⚪ |
| `KC__EMAIL__FROM_ADDRESS` | Dirección de correo del remitente (opcional) | ⚪ |
| `KC__EMAIL__FROM_NAME` | Nombre visible del remitente (opcional) | ⚪ |
| `KC__EMAIL__REQUIREMENT_RECIPIENT` | Correo receptor para solicitudes de requisitos (opcional; necesario para recibir envíos desde la página inicial) | ⚪ |
| `APP_BASE_URL` | URL pública del frontend de este despliegue; obligatoria con SMTP y antes de habilitar invitaciones públicas | Condicional |
| `KC__DEFAULT_ADMIN_EMAIL` | Correo del administrador por defecto | ⚪ |
| `KC__DEFAULT_ADMIN_PASSWORD` | Contraseña inicial de un solo uso: solo obligatoria al crear el primer administrador `system` en producción; no predeterminada/no vacía y al menos 12 caracteres | Condicional |

---

## API

### API compatible con OpenAI

```bash
# Chat Completions (streaming + no streaming)
curl http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer sk-xxx" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-chat",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'

# Listar modelos disponibles
curl http://localhost:3000/v1/models \
  -H "Authorization: Bearer sk-xxx"
```

### API de administración

| Categoría | Endpoint | Descripción |
|:---|:---|:---|
| Auth | `POST /api/v1/auth/register` | Registro de usuario |
| | `POST /api/v1/auth/login` | Inicio de sesión |
| | `POST /api/v1/auth/forgot-password` | Olvidé mi contraseña |
| Usuario | `GET /api/v1/me` | Información del usuario actual |
| | `GET/POST /api/v1/keys` | Gestión de API Keys |
| Facturación | `GET /api/v1/usage` | Estadísticas de uso |
| | `GET /api/v1/billing/records` | Registros de facturación |
| Pago | `POST /api/v1/payments/orders` | Crear orden de pago |
| | `GET /api/v1/payments/balance` | Consulta de saldo |
| Distribución | `GET /api/v1/me/distribution/earnings` | Ganancias de distribución |
| Nodo | `GET /api/v1/me/node-gateway/token` | Token de nodo |
| | `GET /api/v1/me/tips` | Resumen de propinas |
| Admin | `GET/POST /api/v1/accounts` | Gestión de cuentas upstream |
| | `GET/POST /api/v1/settings` | Configuración del sistema |
| | `GET/POST /api/v1/pricing` | Gestión de precios |
| | `GET /api/v1/admin/monitoring/overview` | Vista general de monitorización |

> Para la documentación completa de la API, consulte las definiciones de rutas en el código fuente del proyecto.

---

## Guía de desarrollo

```bash
# Compilar (excluir escritorio y móvil)
cargo build --workspace --exclude desktop --exclude mobile --verbose

# Ejecutar pruebas unitarias
cargo test --lib --workspace --exclude desktop --exclude mobile --verbose

# Ejecutar pruebas de integración
cargo test --package integration-tests --tests --verbose

# Ejecutar pruebas del cliente API frontend
cargo test --package client-api --tests --verbose

# Verificaciones de código Clippy
cargo clippy --workspace --exclude desktop --exclude mobile --all-targets --all-features --future-incompat-report -- -D warnings

# Verificación de formato de código
cargo fmt --all --check

# Compilación de lanzamiento
cargo build -p keycompute-server --release
```

---

## Contribuciones

Damos la bienvenida a todo tipo de contribuciones. Consulta [CONTRIBUTING.md](CONTRIBUTING.md) para saber cómo participar.

- 🐛 [Reportar bugs](https://github.com/keycompute/keycompute/issues/new?template=bug_report.yml)
- 💡 [Solicitar funcionalidades](https://github.com/keycompute/keycompute/issues/new?template=feature_request.yml)
- 🔧 [Enviar código](CONTRIBUTING.md)

---

## Licencia

Este proyecto se distribuye bajo la licencia [MIT](LICENSE).

---

<div align="center">

### 💖 Gracias por usar KeyCompute

Si este proyecto te resulta útil, te agradeceremos una ⭐️.

**[Inicio rápido](#inicio-rápido)** • **[Reportar problemas](https://github.com/keycompute/keycompute/issues)** • **[Últimas versiones](https://github.com/keycompute/keycompute/releases)**

</div>
