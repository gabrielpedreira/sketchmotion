# Plano de execução — fases e etapas

Este documento traduz o roadmap ([`05-roadmap-mvp.md`](./05-roadmap-mvp.md)) em
passos concretos e pequenos, na ordem em que serão construídos. Cada etapa segue
o mesmo formato: objetivo → o que será feito → como validar → só então a
próxima etapa começa.

Cada fase corresponde a uma versão do roadmap (Fase 1 = v0.1, Fase 2 = v0.2...).
**A Fase 1 está detalhada por completo, porque é onde vamos começar agora.** As
fases seguintes têm só um esboço de etapas — serão detalhadas de verdade quando
chegarmos nelas, para não planejar em cima de suposições que podem mudar com o
que aprendermos na Fase 1.

---

## Fase 1 — Fundação (v0.1)

### Etapa 1 — Workspace Cargo vazio
- **Objetivo**: ter o esqueleto do projeto compilando, sem lógica nenhuma.
- **O que será feito**: criar o `Cargo.toml` raiz (workspace) e um `Cargo.toml`
  por crate (`core`, `render`, `tools`, `input`, `animation`, `rigging`,
  `color`, `brush`, `io`, `app`), cada um só com um `lib.rs`/`main.rs` vazio.
- **Validação**: `cargo build` no workspace inteiro compila sem erros.

### Etapa 2 — Spike: skia-safe dentro do egui
- **Objetivo**: provar que dá para desenhar com `skia-safe` e mostrar o
  resultado numa janela `egui`/`eframe`, antes de construir qualquer coisa em
  cima disso.
- **O que será feito**: uma janela mínima que renderiza uma imagem estática
  (ex: um retângulo colorido) com skia e exibe como textura dentro do egui.
- **Validação**: a janela abre e mostra o desenho gerado pelo skia. Se a
  performance ou a integração se mostrarem problemáticas aqui, discutimos
  alternativas **antes** de seguir — é o ponto de maior risco técnico do
  projeto (ver nota em [`02-stack-tecnologica.md`](./02-stack-tecnologica.md)).

### Etapa 3 — Modelo mínimo do `core`
- **Objetivo**: ter `Document`, `Layer` e `Frame` como estruturas de dados,
  sem nenhuma UI ainda.
- **O que será feito**: structs básicas + testes unitários (criar documento,
  adicionar camada, adicionar frame).
- **Validação**: `cargo test` no crate `core` passa.

### Etapa 4 — Canvas interativo
- **Objetivo**: desenhar de verdade com base no mouse, usando o pipeline
  validado na Etapa 2 e os dados da Etapa 3.
- **O que será feito**: capturar posição/clique do mouse (via `input`),
  traduzir em um `Command` simples ("pintar pixel/traço"), aplicar no `core`,
  redesenhar via `render`.
- **Validação**: consigo clicar e arrastar o mouse na janela e ver um traço
  aparecer no canvas.

### Etapa 5 — Ferramentas lápis e borracha
- **Objetivo**: ter as duas primeiras ferramentas reais, no crate `tools`.
- **O que será feito**: máquina de estado simples por ferramenta, alternância
  entre elas.
- **Validação**: alternar entre lápis e borracha e ver o efeito correto no
  canvas.

### Etapa 6 — Seletor de cor simples
- **Objetivo**: escolher a cor usada pela ferramenta lápis.
- **O que será feito**: paleta fixa pequena (crate `color`) + clique para
  selecionar.
- **Validação**: desenhar com mais de uma cor no mesmo documento.

### Etapa 7 — Salvar e carregar projeto
- **Objetivo**: fechar o ciclo completo do MVP.
- **O que será feito**: serialização do `Document` (crate `io`, via
  `serde`/`ciborium`) para um arquivo `.sketchmotion`.
- **Validação** (critério de saída da Fase 1): abrir o app, desenhar um
  traço, salvar, fechar, reabrir, e ver o traço intacto.

### Etapa 8 — Spike opcional: pressão de caneta
- **Objetivo**: validar cedo se `winit` expõe pressão de mesa digitalizadora
  de forma utilizável na(s) plataforma(s) alvo.
- **O que será feito**: protótipo isolado (fora do app principal) só para ler
  e imprimir valores de pressão de um dispositivo real.
- **Validação**: sabemos, com dados reais, se dá para seguir com `winit` puro
  ou se vamos precisar de algo complementar. Pode rodar em paralelo às Etapas
  4–7, já que não bloqueia o critério de saída da Fase 1.

---

## Fase 2 — Camadas e edição (v0.2) — esboço
- Sistema de camadas completo no `core` (criar, excluir, reordenar, ocultar,
  opacidade).
- Seleção retangular + copiar/colar (crate `tools`).
- Transformações básicas (mover, redimensionar).
- Undo/redo funcional (`core::history`).
*Detalhamento em etapas só quando a Fase 1 estiver concluída e validada.*

## Fase 3 — Animação / timeline (v0.3) — esboço
- Múltiplos frames por camada, FPS configurável, reprodução.
- Onion skin.
*Detalhamento em etapas só quando chegarmos aqui.*

## Fase 4 — Exportação de animação (v0.4) — esboço
- Exportar GIF animado, sequência de PNG, frame único.

## Fase 5 — Rigging (v0.5) — esboço
- Pontos de articulação, ossos, hierarquia pai/filho (FK), poses.

---

## Como vamos trabalhar cada etapa

Para cada etapa dentro da Fase 1:
1. Explico o objetivo e a arquitetura envolvida.
2. Explico as tecnologias/crates usadas naquele ponto específico.
3. Mostro a estrutura de arquivos envolvida.
4. Implemento a parte funcional da etapa (código pequeno, revisável).
5. Explico o código.
6. Mostro como testar/validar.
7. Aponto problemas ou riscos identificados.
8. Só avançamos para a próxima etapa depois da sua validação.
