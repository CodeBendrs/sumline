"""API and real-sample regression checks against an explicitly supplied local preview.

Run it against an instance WITHOUT SMTP_URL/MAIL_FROM. With outbound mail configured, accounts
must verify their email before accepting invitations, which these checks do not do; that path is
covered by the Rust tests in src/accounts.rs."""
from pathlib import Path
from urllib.request import Request, build_opener, HTTPCookieProcessor
from urllib.error import HTTPError
from http.cookiejar import CookieJar
import json, os, subprocess, time, uuid, zipfile, io, tempfile
import xml.etree.ElementTree as ET

ORIGIN=os.environ.get('INVOICE_TEST_ORIGIN','http://localhost:3030')
SAMPLES=Path(os.environ['INVOICE_SAMPLE_DIR']) if os.environ.get('INVOICE_SAMPLE_DIR') else None

class Client:
    def __init__(self):
        self.opener=build_opener(HTTPCookieProcessor(CookieJar()));self.csrf=''
    def request(self,path,body=None,method=None,expected=200,csrf=True,extra_headers=None):
        headers={'Origin':ORIGIN,**(extra_headers or {})}
        if csrf:headers['X-CSRF-Token']=self.csrf
        if body is not None:headers['Content-Type']='application/json';body=json.dumps(body).encode()
        req=Request(ORIGIN+path,data=body,method=method or ('POST' if body is not None else 'GET'),headers=headers)
        try:
            response=self.opener.open(req);status=response.status;data=response.read()
        except HTTPError as error:status=error.code;data=error.read()
        assert status==expected,(path,status,data[:300])
        return json.loads(data) if data[:1] in (b'{',b'[') else data
    def create(self,name):
        email=f'{uuid.uuid4().hex}@example.test'
        policy=self.request('/api/privacy')
        body={'email':email,'password':'testing password long enough','store_name':name,'consent':True,'policy_version':policy['version']}
        self.request('/api/auth/register',{**body,'consent':False},expected=400)
        self.request('/api/auth/register',body);me=self.request('/api/me');self.csrf=me['csrf'];return me,email
    def upload(self,files):
        boundary='invoice-test-'+uuid.uuid4().hex;chunks=[]
        for name,data in files:
            chunks.extend([f'--{boundary}\r\nContent-Disposition: form-data; name="files"; filename="{name}"\r\nContent-Type: application/pdf\r\n\r\n'.encode(),data,b'\r\n'])
        chunks.append(f'--{boundary}--\r\n'.encode())
        req=Request(ORIGIN+'/api/upload',data=b''.join(chunks),headers={'Origin':ORIGIN,'X-CSRF-Token':self.csrf,'Content-Type':'multipart/form-data; boundary='+boundary})
        return json.loads(self.opener.open(req).read())

def pdf(lines):
    def literal(text):return text.replace('\\','\\\\').replace('(','\\(').replace(')','\\)')
    stream='BT /F1 12 Tf 50 760 Td '+ ' '.join(('0 -22 Td ' if i else '')+'('+literal(line)+') Tj' for i,line in enumerate(lines))+' ET'
    objects=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',f'<< /Length {len(stream.encode())} >>\nstream\n{stream}\nendstream'.encode()]
    output=b'%PDF-1.4\n';offsets=[0]
    for number,obj in enumerate(objects,1):offsets.append(len(output));output+=f'{number} 0 obj\n'.encode()+obj+b'\nendobj\n'
    start=len(output);output+=f'xref\n0 {len(objects)+1}\n0000000000 65535 f \n'.encode()
    output+=b''.join(f'{offset:010d} 00000 n \n'.encode() for offset in offsets[1:])
    output+=f'trailer\n<< /Size {len(objects)+1} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode();return output

def wait(client,count):
    for _ in range(80):
        records=client.request('/api/invoices')['invoices']
        if len(records)>=count and all(r['status']!='processing' for r in records):return records
        time.sleep(.25)
    raise AssertionError('Processing did not finish')

def run():
    a=Client();b=Client();a.request('/api/invoices',expected=401)
    me_a,email_a=a.create('Store A');me_b,email_b=b.create('Store B');store_a=me_a['stores'][0]['id'];store_b=me_b['stores'][0]['id']
    a.request('/api/stores/'+store_a,{'name':'Store A','identifiers':['2500 Douglas']})
    second=a.request('/api/stores',{'name':'Store A second','identifiers':['999 Main Street']})['stores']
    second_id=next(s['id'] for s in second if s['name']=='Store A second')
    a.request('/api/stores',{'name':'No CSRF','identifiers':[]},expected=403,csrf=False)
    b.request('/api/workspaces/'+me_a['workspace']+'/select',method='POST',expected=404)
    fixtures=[('one.pdf',pdf(['ACME Supplies','Invoice Number: STORE-A-123','Date: 2026-10-02','Due Date: 2026-11-01','Ship to: 2500 Douglas','CAD','SUBTOTAL 100.00','TOTAL TAX 5.00','TOTAL 105.00'])),('two.pdf',pdf(['ACME Supplies','Invoice Number: STORE-B-456','Date: 2026-10-02','Due Date: 2026-11-01','Ship to: 999 Main Street','CAD','SUBTOTAL 200.00','TOTAL TAX 10.00','TOTAL 210.00']))]
    result=a.upload(fixtures);assert len(result['files'])==2
    records=wait(a,2);assert {r['store_id'] for r in records}=={store_a,second_id},records
    id_a=records[0]['id'];b.request('/api/invoices/'+id_a+'/document',expected=404)
    assert b.request('/api/invoices')['invoices']==[]
    a.request('/api/invoices/'+id_a+'/approve',{'supplier':'ACME Supplies','invoice_number':'CORRECTED-1','invoice_date':'2026-10-02','due_date':'2026-11-01','currency':'CAD','subtotal':'200.00','tax':'10.00','total':'210.00','balance':'','po_number':'008142','store_id':store_b},expected=400)
    for record in records:
        i=record['invoice'];body={'supplier':'=UNTRUSTED()','invoice_number':i['invoice_number'],'invoice_date':'2026-10-02','due_date':'2026-11-01','currency':'CAD','subtotal':f'{i["subtotal_cents"]/100:.2f}','tax':f'{i["tax_cents"]/100:.2f}','total':f'{i["total_cents"]/100:.2f}','balance':'','po_number':'008142','store_id':record['store_id'],'revision':record['revision']}
        path='/api/invoices/'+record['id']+'/approve';headers={'Idempotency-Key':str(uuid.uuid4())}
        saved=a.request(path,body,extra_headers=headers);assert saved==a.request(path,body,extra_headers=headers)
        a.request(path,{**body,'supplier':'Different intent'},expected=409,extra_headers=headers)
        a.request(path,body,expected=409,extra_headers={'Idempotency-Key':str(uuid.uuid4())})
    page=a.request('/api/invoices?page=1&from=2026-10-01&to=2026-10-31&sort=total_high');assert page['total']==2 and page['currencies'][0]['total_cents']==31500
    assert page['invoices'][0]['invoice']['total_cents']==21000
    a.request('/api/invoices?from=2026-11-01&to=2026-10-01',expected=400)
    assert a.request('/api/invoices?from=2026-09-01&to=2026-09-30')['total']==0
    assert a.request('/api/invoices?page=999')['invoices']==[]
    b.request('/api/invoices?store='+store_a,expected=404)
    duplicate=a.upload(fixtures);assert all(f['duplicate'] for f in duplicate['files'])
    workbook=a.request('/api/export');archive=zipfile.ZipFile(io.BytesIO(workbook));xml=''.join(archive.read(name).decode() for name in archive.namelist() if name.startswith('xl/worksheets/sheet'))
    assert '<f>' not in xml,'Invoice text became a formula'
    strings=archive.read('xl/sharedStrings.xml').decode()
    assert '=UNTRUSTED()' in strings
    assert 'Store A second' in strings
    # Invoice dates stay sortable dates in Excel, and the approved header style survives tables.
    ns={'s':'http://schemas.openxmlformats.org/spreadsheetml/2006/main'}
    sheet=ET.fromstring(archive.read('xl/worksheets/sheet1.xml'))
    for ref in ('C2','D2'):
        cell=sheet.find(f'.//s:c[@r="{ref}"]',ns)
        assert cell is not None and cell.attrib.get('t') not in ('s','inlineStr'),ref
        assert float(cell.find('s:v',ns).text)>40000,ref
    styles=archive.read('xl/styles.xml').decode()
    assert 'd mmm yyyy' in styles and 'FF225447' in styles
    assert '[Red]' not in styles
    filtered=a.request('/api/export?store='+store_a);assert filtered[:2]==b'PK'
    b.request('/api/export?store='+store_a,expected=404)
    october=a.request('/api/export?from=2026-10-01&to=2026-10-31');assert october[:2]==b'PK'
    september=a.request('/api/export?from=2026-09-01&to=2026-09-30');empty_strings=zipfile.ZipFile(io.BytesIO(september)).read('xl/sharedStrings.xml').decode();assert 'STORE-A-123' not in empty_strings and 'STORE-B-456' not in empty_strings
    # An invitation must match the signed-in email. A member can read shared invoices but cannot manage connections/stores.
    token=a.request('/api/invitations',{'email':email_b})['url'].split('invite=')[1]
    a.request('/api/invitations/accept',{'token':token},expected=400)
    b.request('/api/invitations/accept',{'token':token});assert len(b.request('/api/invoices')['invoices'])==2
    b.request('/api/stores',{'name':'Unauthorized','identifiers':[]},expected=403)
    b.request('/api/connections/google/start',{'consent':True,'policy_version':a.request('/api/privacy')['version']},expected=403)
    b.request('/api/invitations/accept',{'token':token},expected=400)
    assert a.request('/api/consents')['consents']
    assert len(a.request('/api/members')['members'])==2
    b.request('/api/members/'+me_a['user']['id']+'/remove',method='POST',expected=403)
    a.request('/api/members/'+me_b['user']['id']+'/remove',method='POST')
    b.request('/api/invoices/'+id_a+'/document',expected=401)
    assert len(a.request('/api/members')['members'])==1
    # Remove only fixtures created by this test, leaving sample/source invoices untouched.
    for record in a.request('/api/invoices')['invoices']:a.request('/api/invoices/'+record['id']+'/delete',method='POST')
    # A single PDF attachment containing invoices for two stores becomes two independently routed records.
    with tempfile.TemporaryDirectory() as directory:
        paths=[]
        for name,data in fixtures:
            path=Path(directory)/name;path.write_bytes(data);paths.append(str(path))
        bundle=Path(directory)/'bundle.pdf'
        subprocess.run(['pdfunite',*paths,str(bundle)],check=True,capture_output=True)
        a.upload([('bundle.pdf',bundle.read_bytes())])
    split=wait(a,3);parents=[r for r in split if r['status']=='bundle'];children=[r for r in split if r['original_document_id']]
    assert len(parents)==1 and len(children)==2,split
    assert {r['store_id'] for r in children}=={store_a,second_id}
    assert all(r['original_document_id']==parents[0]['id'] for r in children)
    a.request('/api/invoices/'+parents[0]['id']+'/delete',method='POST')
    assert all(r['original_document_id'] is None for r in a.request('/api/invoices')['invoices'])
    for record in children:a.request('/api/invoices/'+record['id']+'/delete',method='POST')
    if SAMPLES:
        expected=[('INVEHPCA124319',79200,3960,83160,'2026-10-02','2026-11-01',1,1),('19829',193980,9700,203680,'2026-09-15','2026-10-15',1,7),('219925',1050926,52546,1103472,'2026-10-02','2026-11-01',2,24),('40989',15500,775,16275,'2026-10-01','2026-10-16',1,1)]
        binary=Path(os.environ.get('INVOICE_TEST_BINARY','target/debug/sumline'))
        for path,values in zip(sorted(SAMPLES.glob('*.pdf')),expected):
            result=subprocess.run([str(binary),'--inspect',str(path)],capture_output=True,text=True,check=True);data=json.loads(result.stdout)
            actual=(data['invoice_number'],data['subtotal_cents'],data['tax_cents'],data['total_cents'],data['invoice_date'],data['due_date'],data['pages'],len(data['items']))
            assert actual==values,(path.name,actual,values)
            if data['invoice_number']!='40989':assert data['warnings']==[],data['warnings']
    print('PASS: filter/date totals and exports, retry idempotency, stale edit conflicts, pagination, isolation, membership roles/removal, store routing, mixed-store PDF splitting, CSRF, consent gate/history, duplicates, safe XLSX, and supplied PDF regression checks')

if __name__=='__main__':run()
