# Estrutura de pastas (proposta)

Cargo workspace com um crate por módulo arquitetural — reforça as fronteiras de
dependência descritas em [`01-arquitetura.md`](./01-arquitetura.md).

```
sketchmotion/
├── Cargo.toml                  # workspace root
├── README.md
├── assets/
│   ├── logo_oficial.png
│   ├── logo_transparente.png
│   ├── logo_escrita.png
│   └── logo-oficial.ico
├── docs/
│   ├── 00-visao-geral.md
│   ├── 01-arquitetura.md
│   ├── 02-stack-tecnologica.md
│   ├── 03-design-system.md
│   ├── 04-estrutura-pastas.md
│   └── 05-roadmap-mvp.md
└── crates/
    ├── core/                   # Document, Layer, Frame, Rig, History
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs
    │       ├── document.rs
    │       ├── layer.rs
    │       ├── frame.rs
    │       ├── rig.rs
    │       └── history.rs
    ├── render/                 # skia-safe: desenha o estado do core
    │   ├── Cargo.toml
    │   └── src/lib.rs
    ├── tools/                  # máquinas de estado por ferramenta
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs
    │       ├── pencil.rs
    │       ├── eraser.rs
    │       └── selection.rs
    ├── input/                  # abstração de mouse/teclado/caneta
    │   ├── Cargo.toml
    │   └── src/lib.rs
    ├── animation/               # timeline, frames, onion skin
    │   ├── Cargo.toml
    │   └── src/lib.rs
    ├── rigging/                 # esqueletos 2D, hierarquia de ossos
    │   ├── Cargo.toml
    │   └── src/lib.rs
    ├── color/
    │   ├── Cargo.toml
    │   └── src/lib.rs
    ├── brush/
    │   ├── Cargo.toml
    │   └── src/lib.rs
    ├── io/                      # save/load/export
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs
    │       ├── project_format.rs
    │       └── export.rs
    └── app/                     # binário final, composição via egui
        ├── Cargo.toml
        └── src/
            ├── main.rs
            └── ui/
                ├── toolbar.rs
                ├── layers_panel.rs
                ├── timeline_panel.rs
                └── canvas_view.rs
```

## Convenções

- Cada crate em `crates/` expõe uma API pública mínima e explícita — nada de
  `pub use *` genérico entre módulos.
- `app` é o único crate que pode depender de todos os outros. Nenhum outro
  crate depende de `app`.
- `core` não depende de nenhum outro crate do workspace (é a base).
- Testes unitários vivem junto ao código (`#[cfg(test)]` nos próprios
  arquivos); testes de integração de cada crate ficam em `crates/<nome>/tests/`.

Esta estrutura é o ponto de partida para a v0.1 (ver
[`05-roadmap-mvp.md`](./05-roadmap-mvp.md)) — crates como `rigging` e
`animation` só ganham conteúdo real a partir da v0.3/v0.5, mas já reservamos o
espaço para não precisar reorganizar o workspace mais tarde.
