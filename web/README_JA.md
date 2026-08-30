# Hardened BliKVM Web インターフェース

この React/TypeScript UI は、`UPSTREAM.md` に固定された Hardened NanoKVM
ベースラインから BliKVM 向けに移植中です。ハードウェアの検出結果だけで
操作を有効にせず、Rust バックエンドが公開した capability のみを表示します。

ビルド手順と安全上の制約は [`README.md`](README.md) を参照してください。
プラットフォームアダプター、復旧手順、実機試験が承認されるまでは、実機へ
この開発ビルドを配置しないでください。
