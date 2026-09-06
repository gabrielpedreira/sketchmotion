# Stack tecnológica

## Linguagem: Rust

Ver decisão e justificativa completa nas conversas do projeto (resumo): performance
equivalente a C++, segurança de memória garantida em tempo de compilação (sem
GC), `cargo` como gerenciador de build/dependências, ecossistema gráfico maduro o
suficiente para este escopo.

## Crates principais por módulo

| Módulo | Crate(s) | Motivo |
|---|---|---|
| Renderização 2D | [`skia-safe`](https://crates.io/crates/skia-safe) | Bindings Rust para Skia — o mesmo motor de renderização 2D do Chrome/Android. Maduro, rápido, com suporte nativo a camadas, blending, paths vetoriais e manipulação de pixels. |
| Interface (UI) | [`egui`](https://crates.io/crates/egui) + [`eframe`](https://crates.io/crates/eframe) | UI em modo imediato, fácil de iterar rapidamente durante o desenvolvimento incremental. `eframe` cuida da janela (via `winit`) e do loop de eventos. |
| Matemática 2D | [`glam`](https://crates.io/crates/glam) | Vetores/matrizes/transformações otimizadas para gráficos 2D/jogos — mais direto que `nalgebra` para o que o rig e as transformações de canvas precisam. |
| Serialização (save format) | [`serde`](https://crates.io/crates/serde) + [`ciborium`](https://crates.io/crates/ciborium) (CBOR) | Formato binário compacto e rápido para o arquivo de projeto próprio, preservando toda a estrutura (camadas, frames, rig). |
| Codecs de imagem | [`image`](https://crates.io/crates/image) | Leitura/escrita de PNG, JPEG e GIF para importação e exportação. |
| Janela/input | `winit` (via `eframe`) | Eventos de mouse, teclado e, com extensão, eventos de caneta/pressão. |

## Pontos em aberto para validar durante a implementação

- **Integração `skia-safe` + `egui`** (risco mais alto, validar primeiro): as
  duas libs não compartilham automaticamente o mesmo pipeline de renderização.
  `egui` desenha via `glow` ou `wgpu`; `skia-safe` tem seu próprio contexto
  (Ganesh/GPU ou raster em CPU). Duas abordagens possíveis: (a) renderizar com
  skia em um buffer e subir como textura para o egui a cada frame — simples,
  mas depende de CPU e pode não escalar para canvas grandes; (b) compartilhar
  o mesmo contexto de GPU entre os dois — mais rápido, porém com poucos
  exemplos de referência no ecossistema Rust atual. Esta é a primeira coisa a
  ser testada na Fase 1, antes de qualquer feature — se não funcionar bem,
  muda a escolha de renderização ou de UI.
- **Suporte a pressão de caneta**: `winit` tem suporte parcial a eventos de
  tablet dependendo da plataforma (Windows Ink, Wintab, ou APIs nativas em
  macOS/Linux). Isso precisa ser validado cedo — é um requisito central do
  projeto (mesa digitalizadora) e pode exigir uma crate complementar ou
  bindings específicos por plataforma. Vou pesquisar e testar isso já na v0.1.
- **GPU para efeitos futuros**: `skia-safe` pode usar backend de GPU (Ganesh).
  Se filtros/efeitos mais pesados forem necessários no futuro, `wgpu` fica
  disponível como complemento, sem precisar trocar o motor de renderização
  principal.

## O que foi descartado e por quê

| Alternativa | Por que não |
|---|---|
| C++ | Mesma performance do Rust, mas com gerenciamento manual de memória — risco maior de bugs (use-after-free, vazamentos) em um projeto solo, incremental e de longo prazo. |
| C# + .NET (Avalonia/SkiaSharp) | Boa produtividade, mas GC introduz picos de latência em cenários de muitas camadas/alta resolução — abaixo do teto de performance que queremos. |
| Web (Canvas2D/WebGL) + Electron/Tauri | Mesmo com Tauri usando Rust por baixo, a renderização ainda passaria por uma WebView — uma camada de abstração a mais entre o app e a GPU/input de caneta, que queremos evitar dado o foco em desempenho máximo. |
