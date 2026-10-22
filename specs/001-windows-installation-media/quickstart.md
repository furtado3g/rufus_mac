# Quickstart validation — M5

Guia para a implementação futura; flags propostas em [contrato](contracts/windows-media.md). Hoje M5 não está implementado. Não interpretar estes comandos como testes já aprovados.

## Pré-requisitos

- Entrega inicial macOS: executar builds/testes nativos no Mac; o usuário compilará Linux/Windows separadamente. Ausência desses ambientes não bloqueia o incremento macOS, mas suas células permanecem não validadas. A mídia criada continua destinada ao instalador Windows UEFI.
- Toolchain do repositório, componentes nativos/recursos privados construídos e verificados pelas receitas que serão adicionadas à implementação.
- ISOs legais Windows 10 22H2 x64 e Windows 11 24H2 x64 como matriz mínima; registrar edição/build/SHA-256 e incluir WIM >4 GiB, ESD grande e conjunto SWM. Ajustes só anunciados na build exata comprovada; Windows 11 ARM64 quando houver ambiente.
- Scratch suficiente para snapshot privado integral da ISO e transformações; arquivo destino de teste pré-dimensionado conforme plano, nunca ISO original. Testar alteração detectável durante captura, alteração posterior sem efeito no snapshot, ENOSPC e limpeza; não tratar snapshot como prova de autenticidade.
- QEMU/OVMF com variáveis separadas por execução, chaves Secure Boot conhecidas e disco virtual descartável para instalação; Windows para leitura independente/chkdsk NTFS.
- Host atual tem QEMU e fsck_msdos; não tem wimlib-imagex, mkntfs, ntfscp ou sgdisk no PATH. Usuário final não deve precisar instalá-los: helpers devem vir no produto.

## 1. Baseline e build

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

Esperado: regressões DD/Linux ausentes. Integração nativa deve ser construída em cada host/arquitetura suportado, não apenas por cargo check cross-compilado.

## 2. Ajuda e análise sem escrita

```sh
cargo run -p image2usb-cli -- write --secure-boot-help
cargo run -p image2usb-cli -- analyze "$WINDOWS_ISO" --json
```

Esperado: ajuda distingue firmware de setup e alerta BitLocker; não abre origem/destino. Análise identifica WIM/ESD/SWM e arquitetura, inclusive UDF e ausência de marcadores textuais.

## 3. Arquivo de teste

Definir `WINDOWS_ISO` e `USB_IMAGE` com caminhos diferentes, exclusivos para teste. Criar imagem vazia de 16 GiB; aumentar se o plano exigir. Não usar caminhos de dispositivo nestes exemplos.

macOS:

```sh
mkfile -n 16g "$USB_IMAGE"
```

Linux:

```sh
truncate -s 16G "$USB_IMAGE"
```

Windows PowerShell:

```powershell
$stream = [System.IO.File]::Open($env:USB_IMAGE, [System.IO.FileMode]::CreateNew)
try { $stream.SetLength(16GB) } finally { $stream.Dispose() }
```

Os exemplos macOS/Linux só devem ser executados com caminho novo de teste; não sobrescrever trabalho existente.

```sh
cargo run -p image2usb-cli -- write "$WINDOWS_ISO" --output "$USB_IMAGE" --mode extract --filesystem auto --dry-run
cargo run -p image2usb-cli -- write "$WINDOWS_ISO" --output "$USB_IMAGE" --mode extract --filesystem fat32 --yes --verify
cargo run -p image2usb-cli -- write "$WINDOWS_ISO" --output "$USB_IMAGE" --mode extract --filesystem ntfs --scheme gpt --yes --verify
```

Esperado: dry-run preserva hash destino; WIM grande gera SWM válidos em FAT32; NTFS preserva arquivo grande e cria partição FAT auxiliar. Repetir NTFS MBR. Auto usa NTFS para ESD grande; FAT32 explícito com ESD grande falha sem modificar destino. Repetir no Windows usando sintaxe `$env:WINDOWS_ISO`/`$env:USB_IMAGE`.

## 4. Testes negativos e integridade

Testes em `crates/iso/tests/synthetic.rs` e `crates/extract/tests/extract.rs` devem cobrir limites FAT32, múltiplos extents, offsets inválidos, paths maliciosos, SWM faltante, helper inválido, scratch insuficiente e cancelamento. Hash integral do destino antes/depois prova ausência de gravação em cada falha de preparação. Testar origem igual ao destino por caminho, hard link e symlink em fixtures descartáveis.

Ler FAT32/NTFS com implementação independente; comparar arquivos e edições com a origem. Em Windows, executar chkdsk sem reparo no volume NTFS de teste somente depois de confirmar a letra atribuída. Nenhum reparo automático é evidência de integridade inicial.

## 5. Opções e GUI

```sh
cargo run -p image2usb-cli -- write "$WINDOWS_ISO" --output "$USB_IMAGE" --mode extract --bypass-tpm --yes --verify
cargo run -p image2usb-cli -- write "$WINDOWS_ISO" --output "$USB_IMAGE" --mode extract --bypass-secure-boot --yes --verify
cargo run -p image2usb-cli -- write "$WINDOWS_ISO" --output "$USB_IMAGE" --mode extract --offline-account --yes --verify
cargo run -p image2usb-gui
```

Executar somente em builds declaradas compatíveis. Comparar padrão, cada opção isolada e combinações. Answer file não seleciona disco, cria senha vazia ou altera firmware. Opção offline deve chegar ao fluxo local com rede conectada/desconectada nos casos anunciados; build desconhecida deve falhar antes da gravação. GUI deve produzir plano equivalente, manter teclado/labels e permitir cancelamento durante preparação.

## 6. Boot e evidências

Usar procedimento QEMU UEFI de `docs/compatibility.md`, imagem como USB e snapshot; fornecer disco virtual separado descartável para SC-002. Para Secure Boot on usar OVMF que efetivamente imponha verificação com DB/DBX conhecidos, não apenas toggle visual. Registrar comandos reais do ambiente; não presumir os mesmos caminhos de firmware em todos os hosts.

Matriz obrigatória: Windows 10/11 x64 × FAT32/NTFS × Secure Boot on/off × VM/físico. Confirmar tela setup; WIM dividido também deve listar mesmas edições e iniciar aplicação em disco virtual descartável. Identificar assinatura de cada EFI e DB/DBX. ARM64 pendente deve permanecer explicitamente pendente.

Gravação física requer autorização específica de um USB descartável, dry-run e confirmação de modelo/serial/capacidade. Nunca usar o Expansion HDD. Alterar Secure Boot manualmente pode reduzir proteção de boot e exigir a chave de recuperação BitLocker; confirmar sua disponibilidade antes de qualquer alteração.

Salvar evidências em `docs/validation/` usando modelo do ROADMAP. Atualizar README, compatibility, ROADMAP e THIRD_PARTY_NOTICES apenas com resultados reais; M5 só fecha com todos os critérios da spec atendidos.
