@echo off
chcp 65001 >nul
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -Command "$ws=New-Object -ComObject WScript.Shell; $lnk=$ws.CreateShortcut([Environment]::GetFolderPath('Desktop')+'\SketchMotion.lnk'); $lnk.TargetPath='%~dp0SketchMotion.bat'; $lnk.WorkingDirectory='%~dp0'; $lnk.IconLocation='%~dp0assets\logo-oficial.ico'; $lnk.Description='Abrir o SketchMotion'; $lnk.Save()"
echo Atalho "SketchMotion" criado na Area de Trabalho.
echo Agora e so dar dois cliques nele para abrir o programa.
pause
