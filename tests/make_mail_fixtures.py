"""Generate fictional invoices for mailbox testing; never use private source documents."""
from pathlib import Path
import argparse, subprocess
from smoke import pdf

parser=argparse.ArgumentParser()
parser.add_argument('--output',type=Path,default=Path('test-fixtures'))
args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
base=['ACME Test Supplies','Date: 2026-10-02','Due Date: 2026-11-01','CAD','SUBTOTAL 100.00','TOTAL TAX 5.00','TOTAL 105.00']
files=[]
for name,number,address in [('cedar.pdf','TEST-CEDAR-001','101 Cedar Avenue'),('maple.pdf','TEST-MAPLE-001','202 Maple Road')]:
    path=args.output/name;path.write_bytes(pdf([base[0],'Invoice Number: '+number,*base[1:3],'Ship to: '+address,*base[3:]]));files.append(path)
subprocess.run(['pdfunite',*(str(path) for path in files),str(args.output/'two-stores.pdf')],check=True)
print('Created fictional test PDFs. Configure store identifiers: 101 Cedar Avenue / 202 Maple Road.')
print('Send cedar.pdf + maple.pdf in one email. Test two-stores.pdf in a separate fresh workspace or delete the earlier test records first to avoid expected duplicate detection.')
