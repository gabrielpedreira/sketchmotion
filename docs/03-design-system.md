# Design system

## Identidade visual

- **Nome**: SketchMotion
- **Logo** (fundo escuro): [`assets/logo_oficial.png`](../assets/logo_oficial.png)
- **Logo** (transparente): [`assets/logo_transparente.png`](../assets/logo_transparente.png)
- **Wordmark**: [`assets/logo_escrita.png`](../assets/logo_escrita.png)
- **Ícone executável**: [`assets/logo-oficial.ico`](../assets/logo-oficial.ico)
  — nota: o `.ico` atual está em 256×228 (não quadrado). Revisar ao empacotar o
  executável, para garantir que todos os tamanhos internos (16/32/48/256px)
  estejam corretos.

## Paleta de cores

Toda a cromática de base da interface usa **cinza neutro puro**, sem nenhum
tingimento de matiz (nem azul, nem quente) — decisão deliberada para não
enviesar a percepção de cor do usuário enquanto ele trabalha, mesmo fora da
área do canvas. É o mesmo princípio usado por Photoshop, Krita e Blender.

| Papel | Cor | Uso |
|---|---|---|
| Canvas / base da janela | `#1B1B1B` | Área de desenho e fundo geral — o tom mais neutro e recuado |
| Timeline | `#262626` | Um degrau acima do canvas, para se diferenciar sem chamar atenção |
| Painéis / toolbar | `#323232` | Elementos de interface permanente |
| Hover / destaque | `#535353` | Reservado para estados de interação — não usar como cor estática |
| Accent (botões, ações) | `#2F84FE` | Única cor com matiz na interface — extraída da logo |
| Seleção (marquee, frame ativo) | `#2FD4FE` | Separado do accent para não ser ambíguo com botões clicáveis |
| Texto primário | `#F2F2F2` | |
| Texto secundário | `#9A9A9A` | |
| Sucesso | `#2FB374` | Feedback de sistema (salvar, exportar com sucesso) |
| Aviso | `#F5A623` | Feedback de sistema |
| Erro | `#FF5C5C` | Feedback de sistema |

**Regra de ouro**: o accent azul é a única cor "quente" (com matiz forte)
permitida na UI. Isso é o que garante que ele se destaque como indicador de
ação — se qualquer outro elemento estático usar cor, o botão perde força
visual.

## Layout de referência

```
┌──────────────────────────────────────────────────┐
│ Arquivo  Editar  Imagem  Camada  Animação  ...    │  ← barra superior
├────────┬──────────────────────────────┬───────────┤
│        │                              │  Camadas  │
│ Ferra- │                              │  Propried.│
│ mentas │           Canvas             │  Cores    │
│        │                              │  Pincéis  │
├────────┴──────────────────────────────┴───────────┤
│  Timeline / Frames / Controles de reprodução       │
└──────────────────────────────────────────────────┘
```
