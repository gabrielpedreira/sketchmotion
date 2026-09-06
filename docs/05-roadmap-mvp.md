# Roadmap incremental

Desenvolvimento em versões pequenas e validadas — cada uma testável e revisável
antes de avançar para a próxima. A ordem pode mudar se, ao longo da
implementação, uma sequência melhor for identificada.

## v0.1 — Fundação
- Janela principal (egui/eframe) com o layout de referência (sem funcionalidade
  ainda nos painéis laterais).
- Canvas básico via `skia-safe` integrado ao egui.
- Criar documento (dimensões, cor de fundo).
- Ferramenta lápis (uma cor, tamanho fixo).
- Borracha.
- Seletor de cor simples.
- Salvar/carregar projeto no formato próprio (`core` + `io` mínimos).
- **Critério de saída**: abrir o app, desenhar um traço, salvar, fechar, reabrir
  e ver o traço intacto.

## v0.2 — Camadas e edição
- Sistema de camadas (criar, excluir, renomear, reordenar, ocultar, opacidade).
- Seleção retangular.
- Copiar/colar.
- Transformações básicas (mover, redimensionar).
- Undo/redo funcional (`core::history`).

## v0.3 — Animação (timeline)
- Timeline com múltiplos frames por camada.
- FPS configurável, reprodução em tempo real.
- Onion skin (frame anterior/posterior, intensidade ajustável).
- Duplicar/excluir/reorganizar frames.

## v0.4 — Exportação de animação
- Exportar GIF animado.
- Exportar sequência de PNG.
- Exportar frame único (PNG/JPEG).

## v0.5 — Rigging (esqueletos 2D)
- Criar pontos de articulação e ossos.
- Hierarquia pai/filho com propagação de transformação (FK).
- Associar imagem/segmento a cada osso.
- Criar poses e usá-las em frames da timeline.

## Depois da v0.5 (backlog, sem ordem fixa)
- Pincéis customizáveis (tamanho, dureza, opacidade dinâmicos com pressão da
  caneta).
- Balde de preenchimento, conta-gotas, formas (linha, retângulo, elipse,
  polígono).
- Seleção livre (lasso) e seleção por cor.
- Suporte validado a pressão de mesa digitalizadora (ver nota em
  [`02-stack-tecnologica.md`](./02-stack-tecnologica.md)).
- Agrupamento de camadas, máscaras, modos de mesclagem.
- IK (inverse kinematics), limites de rotação, keyframes e interpolação
  automática no rig.
- Exportação SVG (se tecnicamente viável).
- Atalhos personalizáveis e layouts configuráveis.

## Por que essa ordem

A sequência prioriza ter, o quanto antes, um **círculo completo e testável**
(desenhar → salvar → reabrir) antes de somar complexidade. Rigging fica por
último entre as funcionalidades centrais porque depende do `core` e do sistema
de camadas/frames já estarem estáveis — mexer na hierarquia de transformações
antes disso multiplicaria o retrabalho.
