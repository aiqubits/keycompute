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

**منصة خدمات حوسبة رموز الذكاء الاصطناعي من الجيل التالي وعالية الأداء**

<p align="center">
  <a href="https://github.com/keycompute/keycompute/stargazers"><img src="https://img.shields.io/github/stars/keycompute/keycompute?style=social" alt="GitHub Stars" /></a>
  <a href="https://github.com/keycompute/keycompute/issues"><img src="https://img.shields.io/github/issues/keycompute/keycompute" alt="GitHub Issues" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License" /></a>
  <a href="./CONTRIBUTING.md"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs Welcome" /></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Rust-1.92%2B-orange?logo=rust" alt="Rust Version" /></a>
</p>

<p align="center">
  <a href="#الميزات">الميزات</a> •
  <a href="#الهندسة-المعمارية">الهندسة المعمارية</a> •
  <a href="#البدء-السريع">البدء السريع</a> •
  <a href="#الإعدادات">الإعدادات</a> •
  <a href="#هيكل-المشروع">هيكل المشروع</a> •
  <a href="#api">API</a> •
  <a href="#دليل-التطوير">التطوير</a>
</p>

</div>

---

## نظرة عامة

KeyCompute هي منصة خدمات حوسبة رموز ذكاء اصطناعي **عالية الأداء** و**قابلة للتوسعة** و**جاهزة للاستخدام مباشرة**. توفر قدرات على مستوى المؤسسات تشمل الوصول الموحد إلى النماذج الكبيرة، والتوجيه الذكي، والقياس والفوترة، وتأجير عقد الحوسبة، والتوزيع متعدد المستويات، وقابلية المراقبة.

> **Pure Rust Full Stack**: الواجهة الخلفية (Axum) + الواجهة الأمامية (Dioxus WASM) + عميل CLI، أنواع ومنطق مشترك، أداء وأمان فائقان.

> **ملاحظة**: هذا المشروع مخصص للتعلم الشخصي فقط. يجب استخدامه بما يتوافق مع [شروط استخدام](https://openai.com/policies) OpenAI ومع القوانين واللوائح المعمول بها. لا تستخدمه لأغراض غير قانونية. ووفقًا للتدابير المؤقتة لإدارة خدمات الذكاء الاصطناعي التوليدي، لا يجوز تقديم أي خدمات ذكاء اصطناعي توليدي غير مسجلة لعامة الجمهور في الصين.

---

## الميزات

### تأجير عقد الحوسبة
عقد الحوسبة تتصل عبر **الاقتراع المسحوب** بدون الحاجة إلى **IP عام**. تشغل النماذج المستضافة على الأجهزة المحلية وتكسب مكافآت بناءً على المساهمات.

- **اتصال بنقرة واحدة**: قم بتشغيل ملف CLI الثنائي المستقل للتسجيل التلقائي → نبضات القلب → استقصاء المهام → التنفيذ المحلي → إرسال النتائج
- **توجيه العقد**: استخدم `node:<اسم_النموذج>` لتوجيه الطلبات صراحة إلى مجموعة العقد
- **تجاوز الفشل التلقائي**: العقد الفاشلة تُستبعد، وتُعاد المهام إلى قائمة الانتظار تلقائيًا
- **استمرارية الجلسة**: جلسات محلية تمنع التسجيل المكرر؛ الإغلاق الآمن يضمن سلامة المهام
- **آلية الإكرامية**: يمكن لأصحاب العقد كسب وسحب الإكراميات

### بوابة موحدة متعددة النماذج
انتقل بسلاسة بين جميع النماذج الرئيسية باستخدام **OpenAI API** القياسي — بسطر واحد فقط:

| Provider | عائلات النماذج | التنفيذ |
|:---|:---|:---:|
| 🟢 OpenAI | GPT-4o / GPT-4 / GPT-3.5 إلخ | ✅ |
| 🟣 Anthropic | Claude 3.5 Sonnet / Opus / Haiku إلخ | ✅ |
| 🔵 Google | Gemini 1.5 / 2.0 Flash / Pro إلخ | ✅ |
| 🔴 DeepSeek | DeepSeek-V3 / R1 / Chat إلخ | ✅ |
| 🟤 Ollama | نماذج محلية (Llama / Qwen / GLM / MiniMax إلخ) | ✅ |
| 🟡 vLLM | نماذج مستضافة ذاتيًا | ✅ |

> GLM (Zhipu) و MiniMax يمكن نشرهما محليًا عبر محول Ollama، وليس كتنفيذات Provider مستقلة.

### محرك التوجيه الذكي
**هندسة توجيه ثنائية الطبقات** مع تسجيل مرجح متعدد العوامل للاختيار الأمثل:

```text
النتيجة = 0.30 × عامل التكلفة + 0.25 × عامل زمن الاستجابة + 0.25 × معدل النجاح + 0.20 × الحالة الصحية
```

- **توجيه على مستوى النموذج** → **توجيه مجمع الحسابات**: توزيع تلقائي بين مقدمي الخدمة والحسابات
- **سلسلة التراجع**: التبديل التلقائي إلى أهداف احتياطية عند فشل الهدف الرئيسي
- **إعادة المحاولة مع التراجع الأسي**: حتى 3 محاولات، 100ms أولي، 10s كحد أقصى
- **وكيل على مستوى الطلب**: دعم وكيل HTTP على مستوى المزود / الحساب / wildcard

### نظام الفوترة والمدفوعات

- **التسوية بعد نهاية التدفق**: حساب دقيق بعد اكتمال الطلب، بدون خصم مسبق، بدون تأثير على النتائج
- **تسعير ثلاثي المستويات**: تسعير خاص بالمستأجر → افتراضي من قاعدة البيانات → احتياطي مشفر (ذاكرة تخزين مؤقت LRU)
- **استخدام دقيق**: أولوية لاستخدام المزود الدقيق، الرجوع إلى تقدير tiktoken
- **شحن الرصيد عبر الإنترنت**: Alipay/WeChat Pay + إدارة الرصيد
- **تحليلات الاستخدام**: تفصيل دقيق لاستهلاك الرموز مع تصور

### نظام الإحالة والتوزيع

- **عمولات الإحالة**: افتراضي 3% للمستوى الأول + 2% للمستوى الثاني، حساب تلقائي
- **روابط الدعوة**: إنشاء روابط دعوة حصرية بنقرة واحدة
- **تكوين مرن**: المسؤولون يضبطون نسب التوزيع عبر API
- **تحليلات الإيرادات**: عرض أرباح الإحالات وقائمة المُحيلين في الوقت الفعلي

### المصادقة والصلاحيات

- **مصادقة مزدوجة**: JWT (جلسات المستخدم) + API Key (`sk-...`، وصول API)
- **فصل الصلاحيات**: API Key حتى مع دور admin لا يمكنها الوصول إلى واجهة الإدارة
- **إدارة كاملة للمستخدمين**: تسجيل → التحقق من البريد → تسجيل الدخول → إعادة تعيين كلمة المرور → إدارة الأدوار
- **تحديد المعدل حسب المجموعات**: تحديد على مستوى المستخدم / المستأجر / API Key (خلفية مزدوجة ذاكرة/Redis)

### قابلية المراقبة

- **مقاييس Prometheus**: حجم الطلبات، زمن الاستجابة، معدل الأخطاء، صحة المزود
- **التتبع الموزع**: Provider Span / Request Span / Stream Span
- **سجلات منظمة**: تنسيق JSON، إخراج متدرج للتطوير/الإنتاج
- **مراقبة المضيف**: مقاييس فورية لوحدة المعالجة / الذاكرة / القرص / الشبكة
- **فحص الصحة**: نقطة `/health` لمراقبة حالة الخدمة بنقرة واحدة

### واجهة أمامية متعددة المنصات

- **لوحة إدارة ويب**: Dioxus WASM SPA
- **سطح المكتب**: تطبيق أصلي Dioxus Desktop
- **الهاتف المحمول**: دعم متعدد المنصات Dioxus Mobile
- **التحكم في الصلاحيات على مستوى المسار**: التحقق من دور Admin، آمن وقابل للإدارة

---

## الهندسة المعمارية

```text
[العميل: Web / Desktop / Mobile (Dioxus)]
                ↕ HTTP/SSE
[طبقة API: keycompute-server (Axum)]
       ├── المصادقة (JWT + API Key)
       ├── تحديد المعدل (ذاكرة/Redis)
       ├── التوجيه (محرك ثنائي الطبقات)
       └── Gateway (طبقة التنفيذ العلوية الوحيدة)
                ↕
[طبقة محولات المزود]
  ├── OpenAI / Anthropic / Google
  ├── DeepSeek
  ├── Ollama (نماذج محلية)
  └── vLLM (مستضافة ذاتيًا)

[شبكة عقد الحوسبة]
  node-token (CLI) ↔ node-gateway ↔ قائمة مهام Redis ↔ استدلال محلي
```

---

## البدء السريع

### المتطلبات

| المكوّن | الإصدار المطلوب |
|:---|:---|
| Rust | ≥ 1.92 |
| Axum | ≥ 0.8.0 |
| Dioxus | ≥ 0.7.1 (لتطوير الواجهة الأمامية) |
| PostgreSQL | ≥ 16 |
| Redis | ≥ 7 (اختياري، لتحديد المعدل الموزع/قائمة مهام العقد) |
| Docker | أحدث إصدار (للنشر عبر الحاويات) |

### الخيار 1: النشر عبر Docker Compose (موصى به)

```bash
# استنساخ المشروع
git clone https://github.com/keycompute/keycompute.git
cd keycompute

# نسخ متغيرات البيئة وتعديلها
cp .env.example .env
# قبل أول تشغيل في الإنتاج، استبدل جميع بيانات الاعتماد التشغيلية.
# يرفض التطبيق التشغيل ما لم تتحقق الشروط التالية على الأقل:
# - KC__AUTH__JWT_SECRET: قيمة غير افتراضية وغير فارغة، بطول 32 بايت على الأقل
# - KC__CRYPTO__SECRET_KEY: ترميز Base64 لـ 32 بايت بالضبط
# - KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET: غير افتراضي، 16 بايت على الأقل
#   (مطلوب هنا لأن Compose يفعّل Redis/Node Gateway)
# - KC__DEFAULT_ADMIN_PASSWORD: غير افتراضية وغير فارغة، 12 محرفًا على الأقل
#   (مطلوبة فقط عند إنشاء أول مدير system)

# تشغيل جميع الخدمات
docker compose up -d

# التحقق من حالة الخدمات
docker compose ps
```

بعد النشر، افتح `http://localhost` (أو المنفذ المحدد في `WEB_PORT`).

ما لم يتم تجاوزها، يكون بريد مدير التهيئة `admin@keycompute.local`. كلمة المرور
هي القيمة المضبوطة في `KC__DEFAULT_ADMIN_PASSWORD`؛ قاعدة بيانات إنتاج جديدة لا
تقبل `change-me-admin-password`. بعد إنشاء أول مدير `system`، احذف كلمة مرور
التهيئة ذات الاستخدام الواحد من البيئة؛ عمليات إعادة التشغيل اللاحقة لا تحتاجها.

### الخيار 2: التطوير المحلي

> ⚠️ **تحذير أمان**: القيم الافتراضية المعروضة أدناه (`change-me-*`) للعرض التوضيحي فقط.
> **لا تستخدمها أبدًا في بيئة الإنتاج!** ولّد كلمات مرور عشوائية قوية باستخدام:
> ```bash
> openssl rand -base64 32
> ```
> يبني `cargo run` العادي ملف debug التنفيذي، لذا تحدد طريقة البدء نمط
> التطوير. يقرأ `config.toml` فقط، ويسمح ببيانات اعتماد محلية عامة والاستماع
> على `0.0.0.0`. لا تتجاوز `.env.example` أو `KC__*` أو `APP_BASE_URL` هذا الملف؛ لا تستخدمه في الإنتاج.

```bash
# إنشاء إعداد cargo run
cp config.example.toml config.toml

# تشغيل PostgreSQL وRedis بمنافذ محلية تطابق config.toml
docker compose --env-file .env.example -f docker-compose.yml -f docker-compose.dev.yml up -d postgres redis

# تثبيت dioxus-cli
curl -sSL http://dioxus.dev/install.sh | sh

# تشغيل الخلفية
cargo run -p keycompute-server

# تشغيل خادم تطوير الواجهة الأمامية (في طرفية أخرى)
dx serve --package web --platform web --hot-reload true --addr 0.0.0.0
```

يحدد `cargo run -p keycompute-server --release` وصورة Docker نمط الإنتاج.
يقرأ ملف release التنفيذي متغيرات البيئة فقط ويتجاهل `config.toml`؛ لا يمكن لمعلم
إعداد أو متغير بيئة تغيير نمط التشغيل.

---

## هيكل المشروع

```text
keycompute/
├── crates/                          # الوحدات الأساسية للخلفية (Rust)
│   ├── keycompute-server/            # خدمة HTTP Axum (تدمج جميع الوحدات)
│   ├── keycompute-types/             # أنواع وماكروات مشتركة
│   ├── keycompute-db/                # ORM قاعدة البيانات وترحيلاتها
│   ├── keycompute-auth/              # المصادقة والتفويض (JWT + API Key + كلمة المرور)
│   ├── keycompute-cache/             # تجريد التخزين المؤقت ودعم Redis
│   ├── keycompute-ratelimit/         # محرك تحديد المعدل (خلفية مزدوجة ذاكرة/Redis)
│   ├── keycompute-pricing/           # محرك التسعير (ثلاثة مستويات + ذاكرة تخزين مؤقت LRU)
│   ├── keycompute-routing/           # محرك التوجيه الذكي ثنائي الطبقات
│   ├── keycompute-runtime/           # وقت التشغيل (تشفير AES-256-GCM + تجريد التخزين)
│   ├── keycompute-billing/           # الفوترة والتسوية (تسوية دقيقة بعد نهاية التدفق)
│   ├── keycompute-distribution/      # نظام الإحالة والتوزيع
│   ├── keycompute-observability/     # ركائز قابلية المراقبة الثلاث
│   ├── keycompute-config/            # محملات منفصلة: TOML لـ debug / البيئة لـ release
│   ├── keycompute-emailserver/       # خدمة البريد الإلكتروني SMTP
│   ├── keycompute-payment/           # تكامل الدفع
│   │   ├── keycompute-alipay/        # دفع Alipay
│   │   └── keycompute-wechatpay/     # دفع WeChat
│   ├── llm-gateway/                  # بوابة تنفيذ LLM (طبقة علوية وحيدة)
│   ├── llm-protocol/                 # ترجمة البروتوكولات وتعريفات مقدمي الخدمة
│   │   ├── openai/                   # بروتوكول متوافق مع OpenAI
│   │   ├── anthropic/                # بروتوكول متوافق مع Anthropic
│   │   └── provider/                 # أنواع بروتوكول مقدمي الخدمة المشتركة
│   ├── node-gateway/                 # بوابة العقد (التسجيل/نبضات القلب/إدارة المهام)
│   └── integration-tests/           # اختبارات تكامل شاملة
├── packages/                         # الواجهة الأمامية (Dioxus 0.7)
│   ├── web/                          # لوحة إدارة ويب
│   ├── ui/                           # مكتبة مكونات UI مشتركة
│   ├── desktop/                      # تطبيق أصلي لسطح المكتب
│   ├── mobile/                       # تطبيق متعدد المنصات للهاتف المحمول
│   └── client-api/                   # تغليف عميل API
├── nginx/                            # إعدادات Nginx كوكيل عكسي
├── Dockerfile.server                 # صورة الخلفية
├── Dockerfile.web                    # صورة الواجهة الأمامية
├── docker-compose.yml                # تنسيق حاويات الإنتاج
├── docker-compose.dev.yml            # منافذ الاعتماديات المحلية
└── docker-compose.replicas.yml       # تنسيق الإنتاج مع نسخ القراءة
```

---

## الإعدادات

### متغيرات البيئة

ينطبق هذا الجدول على بدء الإنتاج release/Docker؛ يستخدم `cargo run` بوضع debug ملف `config.toml`.

| المتغير | الوصف | مطلوب |
|:---|:---|:---:|
| `KC__DATABASE__URL` | سلسلة اتصال PostgreSQL | ✅ |
| `KC__AUTH__JWT_SECRET` | الإنتاج: سر JWT غير افتراضي وغير فارغ بطول 32 بايت على الأقل | ✅ |
| `KC__CRYPTO__SECRET_KEY` | الإنتاج: ترميز Base64 لـ 32 بايت بالضبط؛ لا يمكن تغييره بعد حفظ مفاتيح Provider API | ✅ |
| `KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET` | الإنتاج مع Redis: سر HMAC غير افتراضي بطول 16 بايت على الأقل؛ يصدر رموز تسجيل للاستخدام مرة واحدة | مشروط |
| `KC__REDIS__URL` | سلسلة اتصال Redis (اختياري؛ بدونه: يتحول محدد المعدل إلى الذاكرة، التخزين المؤقت يصبح no-op، بوابة العقدة غير متوفرة) | ⚪ |
| `KC__EMAIL__SMTP_HOST` | مضيف SMTP (اختياري) | ⚪ |
| `KC__EMAIL__SMTP_PORT` | منفذ SMTP (اختياري) | ⚪ |
| `KC__EMAIL__SMTP_USERNAME` | اسم مستخدم SMTP (اختياري) | ⚪ |
| `KC__EMAIL__SMTP_PASSWORD` | كلمة مرور SMTP (اختياري) | ⚪ |
| `KC__EMAIL__FROM_ADDRESS` | عنوان بريد المرسل (اختياري) | ⚪ |
| `KC__EMAIL__FROM_NAME` | الاسم الظاهر للمرسل (اختياري) | ⚪ |
| `KC__EMAIL__REQUIREMENT_RECIPIENT` | بريد استلام طلبات المتطلبات (اختياري؛ مطلوب لاستلام طلبات الصفحة الرئيسية) | ⚪ |
| `APP_BASE_URL` | عنوان الواجهة العام لهذا النشر؛ مطلوب عند تفعيل SMTP وقبل تفعيل روابط الدعوة العامة | مشروط |
| `KC__DEFAULT_ADMIN_EMAIL` | بريد المدير الافتراضي | ⚪ |
| `KC__DEFAULT_ADMIN_PASSWORD` | كلمة مرور تهيئة لمرة واحدة: مطلوبة فقط عند إنشاء أول مدير `system` في الإنتاج؛ غير افتراضية وغير فارغة وبطول 12 محرفًا على الأقل | مشروط |

---

## API

### API متوافقة مع OpenAI

```bash
# Chat Completions (تدفق + غير تدفق)
curl http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer sk-xxx" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-chat",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'

# عرض النماذج المتاحة
curl http://localhost:3000/v1/models \
  -H "Authorization: Bearer sk-xxx"
```

### API الإدارة

| التصنيف | Endpoint | الوصف |
|:---|:---|:---|
| المصادقة | `POST /api/v1/auth/register` | تسجيل مستخدم |
| | `POST /api/v1/auth/login` | تسجيل الدخول |
| | `POST /api/v1/auth/forgot-password` | نسيت كلمة المرور |
| المستخدم | `GET /api/v1/me` | معلومات المستخدم الحالي |
| | `GET/POST /api/v1/keys` | إدارة مفاتيح API |
| الفوترة | `GET /api/v1/usage` | إحصاءات الاستخدام |
| | `GET /api/v1/billing/records` | سجلات الفوترة |
| الدفع | `POST /api/v1/payments/orders` | إنشاء طلب دفع |
| | `GET /api/v1/payments/balance` | استعلام الرصيد |
| التوزيع | `GET /api/v1/me/distribution/earnings` | أرباح التوزيع |
| العقدة | `GET /api/v1/me/node-gateway/token` | رمز العقدة |
| | `GET /api/v1/me/tips` | ملخص الإكراميات |
| الإدارة | `GET/POST /api/v1/accounts` | إدارة الحسابات العلوية |
| | `GET/POST /api/v1/settings` | إعدادات النظام |
| | `GET/POST /api/v1/pricing` | إدارة التسعير |
| | `GET /api/v1/admin/monitoring/overview` | نظرة عامة على المراقبة |

> للحصول على وثائق API كاملة، راجع تعريفات المسارات في الكود المصدري للمشروع.

---

## دليل التطوير

```bash
# البناء (استبعاد سطح المكتب والهاتف المحمول)
cargo build --workspace --exclude desktop --exclude mobile --verbose

# تشغيل اختبارات الوحدة
cargo test --lib --workspace --exclude desktop --exclude mobile --verbose

# تشغيل اختبارات التكامل
cargo test --package integration-tests --tests --verbose

# تشغيل اختبارات عميل API الأمامي
cargo test --package client-api --tests --verbose

# فحوصات الكود Clippy
cargo clippy --workspace --exclude desktop --exclude mobile --all-targets --all-features --future-incompat-report -- -D warnings

# التحقق من تنسيق الكود
cargo fmt --all --check

# بناء الإصدار
cargo build -p keycompute-server --release
```

---

## المساهمة

نرحب بجميع أنواع المساهمات. يرجى مراجعة [CONTRIBUTING.md](CONTRIBUTING.md) لمعرفة كيفية المشاركة.

- 🐛 [الإبلاغ عن الأخطاء](https://github.com/keycompute/keycompute/issues/new?template=bug_report.yml)
- 💡 [طلب ميزات جديدة](https://github.com/keycompute/keycompute/issues/new?template=feature_request.yml)
- 🔧 [إرسال الكود](CONTRIBUTING.md)

---

## الترخيص

هذا المشروع متاح بموجب ترخيص [MIT](LICENSE).

---

<div align="center">

### 💖 شكرًا لاستخدام KeyCompute

إذا كان هذا المشروع مفيدًا لك، فسنكون ممتنين لمنحه ⭐️.

**[البدء السريع](#البدء-السريع)** • **[الإبلاغ عن المشكلات](https://github.com/keycompute/keycompute/issues)** • **[أحدث الإصدارات](https://github.com/keycompute/keycompute/releases)**

</div>
