# SketchMotion — visão geral

## O que é

Software desktop pessoal para criação, edição e animação 2D, reunindo em uma única
ferramenta recursos encontrados em Aseprite, Piskel, Krita, OpenToonz, Pencil2D,
Synfig e Adobe Illustrator: desenho, pixel art, camadas, animação quadro a quadro e
rigging 2D com hierarquia de ossos.

## Objetivos do software

- Criar desenhos e ilustrações 2D.
- Criar animações quadro a quadro (frame-by-frame), com timeline, onion skin e FPS
  configurável.
- Criar animações usando esqueletos/rigs 2D com hierarquia de ossos.
- Trabalhar com múltiplas camadas (criar, ocultar, bloquear, agrupar, opacidade).
- Editar, recortar, selecionar, redimensionar e transformar elementos.
- Exportar em diferentes formatos (PNG, GIF, sequência de PNG e formato próprio).
- Suportar mesa digitalizadora e caneta (pressão, tamanho e opacidade dinâmicos).

## Decisões já tomadas

| Decisão | Escolha | Por quê (resumo) |
|---|---|---|
| Plataforma | Desktop nativo | Prioridade em desempenho e qualidade sobre alcance web |
| Linguagem | Rust | Performance equivalente a C++, sem os riscos clássicos de memória manual; bom ecossistema gráfico maduro |
| Renderização | skia-safe | Mesmo motor 2D do Chrome/Android; maduro, rápido, focado em 2D |
| Interface | egui | Imediata, rápida de iterar, boa para prototipagem de ferramentas de canvas |
| Nome do projeto | SketchMotion | Definido a partir da identidade visual criada |

Justificativas completas em [`01-arquitetura.md`](./01-arquitetura.md) e
[`02-stack-tecnologica.md`](./02-stack-tecnologica.md).

## Como este projeto é conduzido

- Documentação, modelos conceituais, protótipos e sketches vêm **antes** de qualquer
  código.
- Cada etapa é validada pelo autor antes de avançar para a próxima.
- Desenvolvimento incremental, por versões (ver [`05-roadmap-mvp.md`](./05-roadmap-mvp.md)).
- Repositório pensado como portfólio: histórico de commits reflete a evolução real
  das decisões e etapas.

## Identidade visual

Logo, wordmark e paleta de cores documentados em
[`03-design-system.md`](./03-design-system.md). Assets em [`/assets`](../assets).
